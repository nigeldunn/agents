# AgentCore CDK Scaffold

AWS deployment is intentionally out of the first milestone. When the
local smoke path is proven, use this directory for a TypeScript CDK app
that owns:

- ECR repository for the runtime image.
- Bedrock AgentCore Runtime and endpoint.
- HTTP bridge for the engine contract.
- DynamoDB status table keyed by `(agent_type, action_id)`.
- IAM roles and Secrets Manager values for bridge/runtime access.

The engine should continue to point at the bridge with:

```toml
[agent_runner]
base_url = "https://bridge.example.com"
```
