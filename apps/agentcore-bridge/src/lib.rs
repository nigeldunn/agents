use std::{
    env,
    net::SocketAddr,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agent_contract::{AgentRunResponse, AgentType, InvocationRequest, RunRequest};
use aws_sdk_bedrockagentcore::primitives::Blob;
use aws_sdk_dynamodb::types::AttributeValue;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use subtle::ConstantTimeEq;
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub listen_addr: SocketAddr,
    pub runtime_arn: String,
    pub qualifier: String,
    pub status_table: String,
    pub status_ttl_secs: u64,
    pub bearer_token: Option<String>,
}

impl BridgeConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen_addr = env::var("BRIDGE_LISTEN_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
            .parse()?;
        let runtime_arn = require_env("AGENTCORE_RUNTIME_ARN")?;
        let qualifier = env::var("AGENTCORE_QUALIFIER").unwrap_or_else(|_| "DEFAULT".to_owned());
        let status_table = require_env("STATUS_TABLE_NAME")?;
        let status_ttl_secs = match env::var("STATUS_TTL_SECS") {
            Ok(value) if !value.is_empty() => value.parse()?,
            _ => 86_400,
        };
        let bearer_token = env::var("AGENTS_BEARER_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());

        Ok(Self {
            listen_addr,
            runtime_arn,
            qualifier,
            status_table,
            status_ttl_secs,
            bearer_token,
        })
    }
}

fn require_env(key: &str) -> anyhow::Result<String> {
    env::var(key).map_err(|_| anyhow::anyhow!("{key} must be set"))
}

#[derive(Clone)]
pub struct AppState {
    pub agentcore: aws_sdk_bedrockagentcore::Client,
    pub ddb: aws_sdk_dynamodb::Client,
    pub runtime_arn: Arc<str>,
    pub qualifier: Arc<str>,
    pub status_table: Arc<str>,
    pub status_ttl_secs: u64,
    pub bearer_token: Option<Arc<str>>,
}

impl AppState {
    pub fn new(
        config: &BridgeConfig,
        agentcore: aws_sdk_bedrockagentcore::Client,
        ddb: aws_sdk_dynamodb::Client,
    ) -> Self {
        Self {
            agentcore,
            ddb,
            runtime_arn: Arc::from(config.runtime_arn.as_str()),
            qualifier: Arc::from(config.qualifier.as_str()),
            status_table: Arc::from(config.status_table.as_str()),
            status_ttl_secs: config.status_ttl_secs,
            bearer_token: config.bearer_token.as_deref().map(Arc::from),
        }
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/run/{agent_type}", post(run_agent))
        .route("/status/{agent_type}/{action_id}", get(status_agent))
        .route("/healthz", get(healthz))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .with_state(state)
}

async fn run_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_type): Path<String>,
    body: Bytes,
) -> Result<Json<AgentRunResponse>, HttpError> {
    authorize(&state, &headers)?;
    let agent_type = parse_agent_type(&agent_type)?;
    let request: RunRequest = parse_json(&body)?;

    tracing::info!(
        agent_type = %agent_type,
        action_id = request.action_id,
        "bridge run start"
    );

    let pk = compose_pk(agent_type, &request.action_id);

    if let Some(cached) = get_cached(&state, &pk).await? {
        tracing::info!(action_id = request.action_id, "returning cached finished response");
        return Ok(Json(cached));
    }

    let invocation = InvocationRequest {
        agent_type,
        action_id: request.action_id.clone(),
        payload: request.payload,
    };
    let invocation_bytes = serde_json::to_vec(&invocation)
        .map_err(|err| HttpError::Internal(format!("serialize invocation: {err}")))?;

    let resp = state
        .agentcore
        .invoke_agent_runtime()
        .agent_runtime_arn(state.runtime_arn.as_ref())
        .qualifier(state.qualifier.as_ref())
        .content_type("application/json")
        .accept("application/json")
        .payload(Blob::new(invocation_bytes))
        .send()
        .await
        .map_err(|err| HttpError::Internal(format!("invoke_agent_runtime: {err}")))?;

    let response_bytes = resp
        .response
        .collect()
        .await
        .map_err(|err| HttpError::Internal(format!("collect agentcore response: {err}")))?
        .into_bytes();

    let parsed: AgentRunResponse = serde_json::from_slice(&response_bytes)
        .map_err(|err| HttpError::Internal(format!("deserialize agentcore response: {err}")))?;

    match write_status(&state, &pk, agent_type, &request.action_id, &response_bytes).await {
        Ok(()) => Ok(Json(parsed)),
        Err(WriteStatusError::AlreadyExists) => match get_cached(&state, &pk).await? {
            Some(cached) => Ok(Json(cached)),
            None => Ok(Json(parsed)),
        },
        Err(WriteStatusError::Other(err)) => Err(err),
    }
}

async fn status_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((agent_type, action_id)): Path<(String, String)>,
) -> Result<Json<AgentRunResponse>, HttpError> {
    authorize(&state, &headers)?;
    let agent_type = parse_agent_type(&agent_type)?;
    let pk = compose_pk(agent_type, &action_id);

    match get_cached(&state, &pk).await? {
        Some(cached) => Ok(Json(cached)),
        None => Err(HttpError::NotFound),
    }
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

fn compose_pk(agent_type: AgentType, action_id: &str) -> String {
    format!("{agent_type}#{action_id}")
}

async fn get_cached(state: &AppState, pk: &str) -> Result<Option<AgentRunResponse>, HttpError> {
    let result = state
        .ddb
        .get_item()
        .table_name(state.status_table.as_ref())
        .key("pk", AttributeValue::S(pk.to_owned()))
        .send()
        .await
        .map_err(|err| HttpError::Internal(format!("ddb get_item: {err}")))?;

    let Some(item) = result.item else {
        return Ok(None);
    };

    let Some(AttributeValue::S(body)) = item.get("body") else {
        return Ok(None);
    };

    let parsed: AgentRunResponse = serde_json::from_str(body)
        .map_err(|err| HttpError::Internal(format!("deserialize cached body: {err}")))?;

    Ok(Some(parsed))
}

enum WriteStatusError {
    AlreadyExists,
    Other(HttpError),
}

async fn write_status(
    state: &AppState,
    pk: &str,
    agent_type: AgentType,
    action_id: &str,
    body_bytes: &[u8],
) -> Result<(), WriteStatusError> {
    let body_str = std::str::from_utf8(body_bytes)
        .map_err(|err| WriteStatusError::Other(HttpError::Internal(format!("body utf8: {err}"))))?
        .to_owned();
    let ttl = unix_seconds() + state.status_ttl_secs;

    let result = state
        .ddb
        .put_item()
        .table_name(state.status_table.as_ref())
        .item("pk", AttributeValue::S(pk.to_owned()))
        .item("body", AttributeValue::S(body_str))
        .item("agent_type", AttributeValue::S(agent_type.to_string()))
        .item("action_id", AttributeValue::S(action_id.to_owned()))
        .item("ttl", AttributeValue::N(ttl.to_string()))
        .condition_expression("attribute_not_exists(pk)")
        .send()
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) => {
            let service_err = err.into_service_error();
            if service_err.is_conditional_check_failed_exception() {
                Err(WriteStatusError::AlreadyExists)
            } else {
                Err(WriteStatusError::Other(HttpError::Internal(format!(
                    "ddb put_item: {service_err}"
                ))))
            }
        }
    }
}

fn parse_agent_type(value: &str) -> Result<AgentType, HttpError> {
    AgentType::from_str(value).map_err(|_| HttpError::NotFound)
}

fn parse_json<T>(body: &[u8]) -> Result<T, HttpError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    serde_json::from_slice(body).map_err(|err| HttpError::InvalidInput(err.to_string()))
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), HttpError> {
    let Some(expected) = &state.bearer_token else {
        return Ok(());
    };

    let Some(actual) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Err(HttpError::Unauthorized);
    };

    if actual.as_bytes().ct_eq(expected.as_bytes()).into() {
        Ok(())
    } else {
        Err(HttpError::Unauthorized)
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug)]
enum HttpError {
    Unauthorized,
    NotFound,
    InvalidInput(String),
    Internal(String),
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Self::NotFound => (StatusCode::NOT_FOUND, "not found".to_owned()),
            Self::InvalidInput(message) => (StatusCode::UNPROCESSABLE_ENTITY, message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };

        let body = Json(serde_json::json!({ "error": message }));
        (status, body).into_response()
    }
}
