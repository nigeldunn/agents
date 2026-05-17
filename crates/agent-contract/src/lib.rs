use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const AGENT_TYPES: &[AgentType] = &[
    AgentType::Triage,
    AgentType::Planner,
    AgentType::Architect,
    AgentType::Coder,
    AgentType::Reviewer,
    AgentType::SecurityReviewer,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentType {
    Triage,
    Planner,
    Architect,
    Coder,
    Reviewer,
    SecurityReviewer,
}

impl AgentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Planner => "planner",
            Self::Architect => "architect",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
            Self::SecurityReviewer => "security_reviewer",
        }
    }
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentType {
    type Err = UnknownAgentType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "triage" => Ok(Self::Triage),
            "planner" => Ok(Self::Planner),
            "architect" => Ok(Self::Architect),
            "coder" => Ok(Self::Coder),
            "reviewer" => Ok(Self::Reviewer),
            "security_reviewer" => Ok(Self::SecurityReviewer),
            other => Err(UnknownAgentType(other.to_owned())),
        }
    }
}

impl Serialize for AgentType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unknown agent type: {0}")]
pub struct UnknownAgentType(pub String);

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("invalid payload for {agent_type}: {source}")]
    InvalidPayload {
        agent_type: AgentType,
        source: serde_json::Error,
    },
    #[error("failed to serialize output: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RunRequest {
    pub action_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct InvocationRequest {
    pub agent_type: AgentType,
    pub action_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentRunResponse {
    Finished {
        output: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost_cents: Option<u64>,
    },
    Running,
}

impl AgentRunResponse {
    pub fn finished(output: Value, cost_cents: Option<u64>) -> Self {
        Self::Finished { output, cost_cents }
    }

    pub fn running() -> Self {
        Self::Running
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TicketRef {
    pub source: String,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TriagePayload {
    pub ticket: TicketRef,
    pub repo: RepoRef,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlannerPayload {
    pub ticket: TicketRef,
    pub repo: RepoRef,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ArchitectPayload {
    pub ticket: TicketRef,
    pub repo: RepoRef,
    pub plan: PlanPayload,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlanPayload {
    pub tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoderPayload {
    pub ticket: TicketRef,
    pub repo: RepoRef,
    pub task_idx: usize,
    pub task: PlanTask,
    pub review_feedback: Option<String>,
    pub total_reviewer_rejections: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReviewPayload {
    pub ticket: TicketRef,
    pub repo: RepoRef,
    pub branch: String,
    pub head_sha: String,
}

pub type SecurityReviewPayload = ReviewPayload;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TriageOutput {
    pub action_id: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub indeterminate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlannerOutput {
    pub action_id: String,
    pub tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlanTask {
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_in_scope: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ArchitectOutput {
    pub action_id: String,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoderOutput {
    pub action_id: String,
    pub task_idx: usize,
    pub patch: Patch,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Patch {
    pub files: Vec<PatchFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReviewerOutput {
    pub action_id: String,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    High,
    Critical,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SecurityFinding {
    pub severity: FindingSeverity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SecurityReviewerOutput {
    pub action_id: String,
    pub passed: bool,
    pub findings: Vec<SecurityFinding>,
}

pub fn validate_payload(agent_type: AgentType, payload: &Value) -> Result<(), ContractError> {
    match agent_type {
        AgentType::Triage => parse_payload::<TriagePayload>(agent_type, payload),
        AgentType::Planner => parse_payload::<PlannerPayload>(agent_type, payload),
        AgentType::Architect => parse_payload::<ArchitectPayload>(agent_type, payload),
        AgentType::Coder => parse_payload::<CoderPayload>(agent_type, payload),
        AgentType::Reviewer => parse_payload::<ReviewPayload>(agent_type, payload),
        AgentType::SecurityReviewer => parse_payload::<SecurityReviewPayload>(agent_type, payload),
    }
}

fn parse_payload<T>(agent_type: AgentType, payload: &Value) -> Result<(), ContractError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value::<T>(payload.clone())
        .map(|_| ())
        .map_err(|source| ContractError::InvalidPayload { agent_type, source })
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_type_round_trips_wire_names() {
        for agent_type in AGENT_TYPES {
            let encoded = serde_json::to_string(agent_type).unwrap();
            assert_eq!(encoded, format!("\"{}\"", agent_type.as_str()));
            let decoded: AgentType = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, *agent_type);
        }
    }

    #[test]
    fn security_severity_uses_contract_values() {
        assert_eq!(
            serde_json::to_string(&FindingSeverity::Info).unwrap(),
            "\"info\""
        );
        assert_eq!(
            serde_json::to_string(&FindingSeverity::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(
            serde_json::to_string(&FindingSeverity::High).unwrap(),
            "\"high\""
        );
        assert_eq!(
            serde_json::to_string(&FindingSeverity::Critical).unwrap(),
            "\"critical\""
        );
    }

    #[test]
    fn validate_payload_rejects_missing_required_fields() {
        let payload = serde_json::json!({
            "ticket": { "source": "manual", "id": "ENG-123" }
        });
        assert!(validate_payload(AgentType::Planner, &payload).is_err());
    }
}
