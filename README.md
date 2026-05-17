# Agents

Rust workspace for agent services that implement the orchestrator
`agent_runner` contract and can later be hosted through Amazon Bedrock
AgentCore.

## Local Mock

The first runnable service is `mock-agent-http`. It is intentionally
deterministic so the engine can validate the whole workflow without an
LLM or AWS deployment.

```sh
cargo run -p mock-agent-http
```

Defaults:

- Listens on `0.0.0.0:8080`.
- Exposes `/run/{agent_type}`, `/status/{agent_type}/{action_id}`, and
  `/healthz` for the engine.
- Exposes `/invocations` and `/ping` as an AgentCore-shaped surface for
  local smoke runs. The request body matches the AgentCore
  `InvokeAgentRuntime` shape, but the response is the engine
  `AgentRunResponse`. A future bridge will translate to the real
  AgentCore event-stream response shape when AWS deployment lands.
- Uses `AGENTS_BEARER_TOKEN` to gate `/run`, `/status`, and
  `/invocations`. `/healthz` and `/ping` are always open so load
  balancers and AgentCore probes can reach them.
- Uses `AGENTS_MOCK_COST_CENTS`, default `50`.

Example engine config:

```toml
[agent_runner]
base_url = "http://localhost:8080"
```

## Workspace Shape

- `crates/agent-contract`: v1 JSON request and response types.
- `crates/agent-runtime`: agent trait, registry, deterministic mock
  agents, and in-memory status store.
- `apps/mock-agent-http`: Axum service for local orchestration smoke
  tests.

## AgentCore Direction

Keep the engine contract stable. A later AWS bridge should expose the
engine HTTP contract and call AgentCore `InvokeAgentRuntime` using the
runtime `/invocations` body:

```json
{
  "agent_type": "triage",
  "action_id": "01J9...XYZ",
  "payload": {}
}
```

Durable production status should live outside the runtime, for example
in DynamoDB keyed by `(agent_type, action_id)`. The local mock keeps
status in memory because the first milestone is a local smoke run.
