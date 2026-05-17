use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use agent_contract::{
    AGENT_TYPES, AgentType, ArchitectOutput, ArchitectPayload, CoderOutput, CoderPayload,
    ContractError, Patch, PatchFile, PlanTask, PlannerOutput, PlannerPayload, ReviewPayload,
    ReviewerOutput, SecurityReviewPayload, SecurityReviewerOutput, TriageOutput, TriagePayload,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("unknown agent type: {0}")]
    UnknownAgentType(String),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("failed to serialize mock output: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("agent failed: {0}")]
    AgentFailed(String),
}

#[derive(Debug, Clone)]
pub struct AgentRequest<'a> {
    pub action_id: &'a str,
    pub payload: &'a Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcome {
    pub output: Value,
    pub cost_cents: Option<u64>,
}

#[async_trait]
pub trait Agent: Send + Sync {
    fn agent_type(&self) -> AgentType;

    async fn run(&self, request: AgentRequest<'_>) -> Result<Value, RuntimeError>;
}

#[derive(Clone)]
pub struct AgentRegistry {
    agents: Arc<HashMap<AgentType, Arc<dyn Agent>>>,
    cost_cents: Option<u64>,
}

impl AgentRegistry {
    pub fn new(agents: impl IntoIterator<Item = Arc<dyn Agent>>, cost_cents: Option<u64>) -> Self {
        let mut by_type = HashMap::new();
        for agent in agents {
            by_type.insert(agent.agent_type(), agent);
        }
        Self {
            agents: Arc::new(by_type),
            cost_cents,
        }
    }

    pub fn mock(cost_cents: Option<u64>) -> Self {
        Self::new(
            AGENT_TYPES
                .iter()
                .copied()
                .map(|agent_type| Arc::new(MockAgent::new(agent_type)) as Arc<dyn Agent>),
            cost_cents,
        )
    }

    pub async fn run(
        &self,
        agent_type: AgentType,
        action_id: &str,
        payload: &Value,
    ) -> Result<AgentOutcome, RuntimeError> {
        let agent = self
            .agents
            .get(&agent_type)
            .ok_or_else(|| RuntimeError::UnknownAgentType(agent_type.to_string()))?;
        let output = agent.run(AgentRequest { action_id, payload }).await?;
        Ok(AgentOutcome {
            output,
            cost_cents: self.cost_cents,
        })
    }
}

fn parse_payload<T>(payload: &Value, agent_type: AgentType) -> Result<T, RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value::<T>(payload.clone())
        .map_err(|source| ContractError::InvalidPayload { agent_type, source }.into())
}

#[derive(Debug, Clone)]
pub struct StoredRun {
    pub output: Value,
    pub cost_cents: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryStatusStore {
    inner: Arc<RwLock<HashMap<StatusKey, StoredRun>>>,
}

impl InMemoryStatusStore {
    pub fn insert(
        &self,
        agent_type: AgentType,
        action_id: impl Into<String>,
        output: Value,
        cost_cents: Option<u64>,
    ) {
        let mut inner = self.inner.write().expect("status store lock poisoned");
        inner.insert(
            StatusKey {
                agent_type,
                action_id: action_id.into(),
            },
            StoredRun { output, cost_cents },
        );
    }

    pub fn get(&self, agent_type: AgentType, action_id: &str) -> Option<StoredRun> {
        let inner = self.inner.read().expect("status store lock poisoned");
        inner
            .get(&StatusKey {
                agent_type,
                action_id: action_id.to_owned(),
            })
            .cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StatusKey {
    agent_type: AgentType,
    action_id: String,
}

#[derive(Debug)]
pub struct MockAgent {
    agent_type: AgentType,
}

impl MockAgent {
    pub const fn new(agent_type: AgentType) -> Self {
        Self { agent_type }
    }
}

#[async_trait]
impl Agent for MockAgent {
    fn agent_type(&self) -> AgentType {
        self.agent_type
    }

    async fn run(&self, request: AgentRequest<'_>) -> Result<Value, RuntimeError> {
        tracing::info!(
            agent_type = %self.agent_type,
            action_id = request.action_id,
            "running mock agent"
        );

        let output = match self.agent_type {
            AgentType::Triage => {
                let _: TriagePayload = parse_payload(request.payload, self.agent_type)?;
                serde_json::to_value(TriageOutput {
                    action_id: request.action_id.to_owned(),
                    accepted: true,
                    indeterminate: false,
                    reason: None,
                })?
            }
            AgentType::Planner => {
                let _: PlannerPayload = parse_payload(request.payload, self.agent_type)?;
                serde_json::to_value(PlannerOutput {
                    action_id: request.action_id.to_owned(),
                    tasks: vec![PlanTask {
                        description: "smoke-test single task".to_owned(),
                        files_in_scope: vec!["src/lib.rs".to_owned()],
                    }],
                })?
            }
            AgentType::Architect => {
                let _: ArchitectPayload = parse_payload(request.payload, self.agent_type)?;
                serde_json::to_value(ArchitectOutput {
                    action_id: request.action_id.to_owned(),
                    accepted: true,
                    feedback: None,
                })?
            }
            AgentType::Coder => {
                let payload: CoderPayload = parse_payload(request.payload, self.agent_type)?;
                serde_json::to_value(CoderOutput {
                    action_id: request.action_id.to_owned(),
                    task_idx: payload.task_idx,
                    patch: Patch {
                        files: vec![PatchFile {
                            path: "src/lib.rs".to_owned(),
                            mode: None,
                            content: Some("pub fn it_works() {}\n".to_owned()),
                        }],
                    },
                    notes: Some("smoke-test patch".to_owned()),
                })?
            }
            AgentType::Reviewer => {
                let _: ReviewPayload = parse_payload(request.payload, self.agent_type)?;
                serde_json::to_value(ReviewerOutput {
                    action_id: request.action_id.to_owned(),
                    passed: true,
                    feedback: None,
                })?
            }
            AgentType::SecurityReviewer => {
                let _: SecurityReviewPayload = parse_payload(request.payload, self.agent_type)?;
                serde_json::to_value(SecurityReviewerOutput {
                    action_id: request.action_id.to_owned(),
                    passed: true,
                    findings: Vec::new(),
                })?
            }
        };

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use agent_contract::AgentRunResponse;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn mock_registry_returns_planner_task() {
        let registry = AgentRegistry::mock(Some(50));
        let payload = json!({
            "ticket": { "source": "manual", "id": "ENG-123" },
            "repo": { "owner": "octo", "name": "world" }
        });

        let outcome = registry
            .run(AgentType::Planner, "01J9XYZ", &payload)
            .await
            .unwrap();

        assert_eq!(outcome.cost_cents, Some(50));
        assert_eq!(outcome.output["action_id"], "01J9XYZ");
        assert_eq!(
            outcome.output["tasks"][0]["description"],
            "smoke-test single task"
        );
        assert_eq!(
            outcome.output["tasks"][0]["files_in_scope"][0],
            "src/lib.rs"
        );

        let response = AgentRunResponse::finished(outcome.output, outcome.cost_cents);
        match response {
            AgentRunResponse::Finished { cost_cents, .. } => assert_eq!(cost_cents, Some(50)),
            AgentRunResponse::Running => panic!("expected Finished, got Running"),
        }
    }

    #[tokio::test]
    async fn mock_registry_rejects_invalid_payload() {
        let registry = AgentRegistry::mock(None);
        let err = registry
            .run(AgentType::Planner, "01J9XYZ", &json!({ "ticket": {} }))
            .await
            .unwrap_err();

        assert!(matches!(err, RuntimeError::Contract(_)));
    }

    #[test]
    fn status_store_keys_by_agent_type_and_action_id() {
        let store = InMemoryStatusStore::default();
        store.insert(AgentType::Triage, "a1", json!({ "ok": true }), Some(7));

        assert!(store.get(AgentType::Planner, "a1").is_none());
        let stored = store.get(AgentType::Triage, "a1").unwrap();
        assert_eq!(stored.output["ok"], true);
        assert_eq!(stored.cost_cents, Some(7));
    }
}
