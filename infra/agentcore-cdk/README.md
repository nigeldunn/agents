# AgentCore CDK

Single-stack CDK app that deploys the agent workspace via Bedrock
AgentCore Runtime, fronted by a Rust HTTP bridge running on Fargate in
the orchestrator's existing VPC.

## Topology

```
Orchestrator (Fargate, existing) → Bridge (Fargate, this stack) → AgentCore Runtime → mock-agent-http container
                                            │
                                            └── DynamoDB (status, TTL'd)
```

The bridge speaks the engine contract (`POST /run/{agent_type}`,
`GET /status/...`, `GET /healthz`) and translates each `/run` into a
`bedrock-agentcore:InvokeAgentRuntime` SDK call. Responses are cached in
DynamoDB so `/status` is a point lookup.

## Resources provisioned

- `AWS::BedrockAgentCore::Runtime` hosting the `mock-agent-http` image
  (linux/arm64, PUBLIC network, HTTP protocol).
- ECS Fargate service + internal ALB for the bridge, in private subnets.
- DynamoDB status table (PK `pk = "{agent_type}#{action_id}"`, TTL'd).
- Secrets Manager secret `agents/bearer-token`, injected into the bridge
  task as `AGENTS_BEARER_TOKEN`. The AgentCore container does **not**
  receive the token — AgentCore Runtime does not forward arbitrary
  headers to the container, so the bridge is the only auth boundary.
- IAM: AgentCore execution role (ECR pull, CloudWatch logs, X-Ray,
  metrics). Bridge task role scoped to `InvokeAgentRuntime` on the
  runtime ARN, DynamoDB R/W on the status table, and `GetSecretValue`
  on the bearer secret.

## Deploy

```bash
cd infra/agentcore-cdk
npm install
npx cdk bootstrap aws://$CDK_DEFAULT_ACCOUNT/$CDK_DEFAULT_REGION
npx cdk deploy \
  -c vpcId=vpc-xxxx \
  -c orchestratorSgId=sg-yyyy \
  -c clusterName=optional-existing-cluster \
  -c createVpcEndpoints=true   # only if VPC has neither NAT nor endpoints
```

Required context: `vpcId`, `orchestratorSgId`.
Optional context: `clusterName`, `createVpcEndpoints`.

If you set `createVpcEndpoints=true`, the stack adds interface endpoints
for `bedrock-agentcore`, Secrets Manager, ECR (api + dkr), and Logs, plus
gateway endpoints for DynamoDB and S3.

## Outputs

- `BridgeAlbDns` — internal ALB DNS for the orchestrator to call.
- `AgentCoreRuntimeArn` — the runtime invoked by the bridge.
- `StatusTableName` — DynamoDB table holding cached responses.
- `BearerSecretArn` — Secrets Manager ARN shared with the orchestrator.

Point the orchestrator at the bridge with:

```toml
[agent_runner]
base_url = "http://<BridgeAlbDns>"
```

and give it the `BearerSecretArn` value (either as
`AGENTS_BEARER_TOKEN` or via the same Secrets Manager reference).

## Verify after deploy

```bash
T=$(aws secretsmanager get-secret-value --secret-id agents/bearer-token \
  --query SecretString --output text)
curl -sS -X POST http://<BridgeAlbDns>/run/triage \
  -H "authorization: Bearer $T" \
  -H "content-type: application/json" \
  -d '{"action_id":"smoke-1","payload":{"ticket":{"source":"manual","id":"ENG-1"},"repo":{"owner":"o","name":"r"}}}'
```

Expect `{"status":"finished","output":{...},"cost_cents":50}`. Repeat the
call to confirm idempotent replay from DynamoDB, or
`GET /status/triage/smoke-1` to read the cached body directly.
