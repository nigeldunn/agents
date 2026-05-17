# AgentCore CDK

Two CDK stacks that together deploy the agent workspace via Bedrock
AgentCore Runtime, fronted by a Rust HTTP bridge running on Fargate in
the orchestrator's existing VPC. Container images are built and pushed
by GitHub Actions; both stacks consume them from ECR by tag.

## Stacks

### `AgentcoreBootstrapStack` (deployed once, manually)

One-shot. Run locally with admin AWS creds before any CI runs. Creates:

- The GitHub Actions OIDC provider.
- IAM role `agents-github-actions-deploy`, trust-scoped to
  `repo:nigeldunn/agents:ref:refs/heads/main`. Permissions: ECR push to
  the two repos below, `sts:AssumeRole` on the CDK bootstrap roles, and
  `cloudformation:DescribeStacks` for read-back.
- ECR repos `agents/mock-agent-http` and `agents/agentcore-bridge` with
  image scanning on push and a 20-image lifecycle policy.

### `AgentcoreStack` (deployed by CI on every push to `main`)

- `AWS::BedrockAgentCore::Runtime` pointing at
  `agents/mock-agent-http:<imageTag>` in ECR (linux/arm64, PUBLIC, HTTP).
- ECS Fargate service running `agents/agentcore-bridge:<imageTag>` in
  private isolated subnets, behind an internal ALB.
- DynamoDB status table (PK `pk = "{agent_type}#{action_id}"`, TTL'd).
- Secrets Manager secret `agents/bearer-token`, injected into the bridge
  task. **Not** injected into the AgentCore container — that container
  trusts AgentCore's IAM boundary.
- VPC interface endpoints for AgentCore data plane, Secrets Manager,
  ECR (api + dkr), CloudWatch Logs, and gateway endpoints for DynamoDB +
  S3, since the private subnets are isolated (no NAT).

## Initial setup (one-shot)

```bash
cd infra/agentcore-cdk
npm install
npx cdk bootstrap aws://<account>/<region>     # if not already bootstrapped

# If the GitHub Actions OIDC provider already exists in this account
# (another project may have created it), import it instead of failing:
EXISTING_OIDC=$(aws iam list-open-id-connect-providers \
  --query "OpenIDConnectProviderList[?contains(Arn,'token.actions.githubusercontent.com')].Arn" \
  --output text)

npx cdk deploy AgentcoreBootstrapStack \
  ${EXISTING_OIDC:+-c existingOidcProviderArn=$EXISTING_OIDC}
```

The bootstrap stack outputs `DeployRoleArn`. If it differs from the
hard-coded value in `.github/workflows/deploy.yml`, update the workflow.

The ECR repositories `agents/mock-agent-http` and `agents/agentcore-bridge`
are imported by name, not owned by the stack. Image scanning + lifecycle
policy are set out-of-band with `aws ecr put-image-scanning-configuration`
and `aws ecr put-lifecycle-policy`.

## Day-to-day: push to main

```bash
git push origin main
```

The workflow:

1. Builds `mock-agent-http` and `agentcore-bridge` images on a native
   `ubuntu-24.04-arm` runner with Buildx (no QEMU).
2. Tags both with the 12-char commit SHA plus `latest`, pushes to ECR.
3. Runs `cdk deploy AgentcoreStack -c imageTag=<sha>`.

The hard-coded values in the workflow:

| Variable | Value |
| --- | --- |
| `AWS_REGION` | `ap-southeast-2` |
| `AWS_ACCOUNT_ID` | `339712920881` |
| `VPC_ID` | `vpc-0da5ae4e455363f5f` |
| `ORCHESTRATOR_SG_ID` | `sg-09a7ab0628444bc36` |
| `CLUSTER_NAME` | `orch-cluster` |
| `CREATE_VPC_ENDPOINTS` | `true` |

Override the deployed image tag with `workflow_dispatch` + `image_tag`.

## Outputs

After `AgentcoreStack` deploys, four CloudFormation outputs:

- `BridgeAlbDns` — internal ALB DNS for the orchestrator to call.
- `AgentCoreRuntimeArn` — the runtime invoked by the bridge.
- `StatusTableName` — DynamoDB table holding cached responses.
- `BearerSecretArn` — Secrets Manager ARN shared with the orchestrator.

Point the orchestrator at the bridge with:

```toml
[agent_runner]
base_url = "http://<BridgeAlbDns>"
```

and give it the secret value (or read it from the same ARN).

## Verify after deploy

```bash
T=$(aws secretsmanager get-secret-value --secret-id agents/bearer-token \
  --query SecretString --output text)
curl -sS -X POST http://<BridgeAlbDns>/run/triage \
  -H "authorization: Bearer $T" \
  -H "content-type: application/json" \
  -d '{"action_id":"smoke-1","payload":{"ticket":{"source":"manual","id":"ENG-1"},"repo":{"owner":"o","name":"r"}}}'
```

Expect `{"status":"finished","output":{...},"cost_cents":50}`. Repeat
the call to confirm idempotent replay from DynamoDB, or
`GET /status/triage/smoke-1` to read the cached body directly.
