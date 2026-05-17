use std::{
    env,
    net::SocketAddr,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agent_contract::{AgentRunResponse, AgentType, InvocationRequest, RunRequest};
use agent_runtime::{AgentRegistry, InMemoryStatusStore, RuntimeError};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use subtle::ConstantTimeEq;
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub listen_addr: SocketAddr,
    pub bearer_token: Option<String>,
    pub mock_cost_cents: Option<u64>,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen_addr = env::var("AGENTS_LISTEN_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
            .parse()?;
        let bearer_token = env::var("AGENTS_BEARER_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        let mock_cost_cents = match env::var("AGENTS_MOCK_COST_CENTS") {
            Ok(value) if !value.is_empty() => Some(value.parse()?),
            _ => Some(50),
        };

        Ok(Self {
            listen_addr,
            bearer_token,
            mock_cost_cents,
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    registry: AgentRegistry,
    status_store: InMemoryStatusStore,
    bearer_token: Option<Arc<str>>,
}

impl AppState {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            registry: AgentRegistry::mock(config.mock_cost_cents),
            status_store: InMemoryStatusStore::default(),
            bearer_token: config.bearer_token.as_deref().map(Arc::from),
        }
    }
}

pub fn build_router(config: AppConfig) -> Router {
    let state = AppState::new(&config);

    Router::new()
        .route("/run/{agent_type}", post(run_agent))
        .route("/status/{agent_type}/{action_id}", get(status_agent))
        .route("/healthz", get(healthz))
        .route("/invocations", post(invoke_agentcore))
        .route("/ping", get(ping))
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
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    let agent_type = parse_agent_type(&agent_type)?;
    let request: RunRequest = parse_json(&body)?;

    match request_id {
        Some(rid) => tracing::info!(
            agent_type = %agent_type,
            action_id = request.action_id,
            request_id = rid,
            "received engine agent run"
        ),
        None => tracing::info!(
            agent_type = %agent_type,
            action_id = request.action_id,
            "received engine agent run"
        ),
    }

    run_and_store(&state, agent_type, &request.action_id, &request.payload).await
}

async fn status_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((agent_type, action_id)): Path<(String, String)>,
) -> Result<Json<AgentRunResponse>, HttpError> {
    authorize(&state, &headers)?;
    let agent_type = parse_agent_type(&agent_type)?;

    match state.status_store.get(agent_type, &action_id) {
        Some(stored) => Ok(Json(AgentRunResponse::finished(
            stored.output,
            stored.cost_cents,
        ))),
        None => Err(HttpError::NotFound),
    }
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn invoke_agentcore(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<AgentRunResponse>, HttpError> {
    authorize(&state, &headers)?;
    let request: InvocationRequest = parse_json(&body)?;

    tracing::info!(
        agent_type = %request.agent_type,
        action_id = request.action_id,
        "received AgentCore-style invocation"
    );

    run_and_store(
        &state,
        request.agent_type,
        &request.action_id,
        &request.payload,
    )
    .await
}

async fn ping() -> Json<PingResponse> {
    Json(PingResponse {
        status: "Healthy",
        time_of_last_update: unix_seconds(),
    })
}

async fn run_and_store(
    state: &AppState,
    agent_type: AgentType,
    action_id: &str,
    payload: &serde_json::Value,
) -> Result<Json<AgentRunResponse>, HttpError> {
    let outcome = state
        .registry
        .run(agent_type, action_id, payload)
        .await
        .map_err(HttpError::from_runtime)?;

    state.status_store.insert(
        agent_type,
        action_id,
        outcome.output.clone(),
        outcome.cost_cents,
    );

    Ok(Json(AgentRunResponse::finished(
        outcome.output,
        outcome.cost_cents,
    )))
}

fn parse_agent_type(value: &str) -> Result<AgentType, HttpError> {
    AgentType::from_str(value).map_err(|_| HttpError::NotFound)
}

fn parse_json<T>(body: &[u8]) -> Result<T, HttpError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    serde_json::from_slice(body).map_err(|source| HttpError::InvalidInput(source.to_string()))
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

#[derive(Debug, Serialize)]
struct PingResponse {
    status: &'static str,
    time_of_last_update: u64,
}

#[derive(Debug)]
enum HttpError {
    Unauthorized,
    NotFound,
    InvalidInput(String),
    Internal(String),
}

impl HttpError {
    fn from_runtime(err: RuntimeError) -> Self {
        match err {
            RuntimeError::UnknownAgentType(_) => Self::NotFound,
            RuntimeError::Contract(err) => Self::InvalidInput(err.to_string()),
            RuntimeError::Serialize(err) => Self::Internal(err.to_string()),
            RuntimeError::AgentFailed(err) => Self::Internal(err),
        }
    }
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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::{Method, Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    fn test_config() -> AppConfig {
        AppConfig {
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            bearer_token: None,
            mock_cost_cents: Some(50),
        }
    }

    fn base_payload() -> Value {
        json!({
            "ticket": { "source": "manual", "id": "ENG-123" },
            "repo": { "owner": "octo", "name": "world" }
        })
    }

    async fn request_json(
        app: Router,
        method: Method,
        uri: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header("x-request-id", "req_018f")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };

        (status, json)
    }

    #[tokio::test]
    async fn run_stores_finished_status() {
        let app = build_router(test_config());
        let body = json!({
            "action_id": "01J9XYZ",
            "payload": base_payload()
        });

        let (status, json) = request_json(app.clone(), Method::POST, "/run/planner", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "finished");
        assert_eq!(json["cost_cents"], 50);
        assert_eq!(
            json["output"]["tasks"][0]["description"],
            "smoke-test single task"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status/planner/01J9XYZ")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn status_returns_404_for_unknown_action() {
        let app = build_router(test_config());
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status/planner/unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unknown_agent_type_returns_404() {
        let app = build_router(test_config());
        let body = json!({
            "action_id": "01J9XYZ",
            "payload": base_payload()
        });

        let (status, _) = request_json(app, Method::POST, "/run/not_real", body).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_payload_returns_422() {
        let app = build_router(test_config());
        let body = json!({
            "action_id": "01J9XYZ",
            "payload": { "ticket": { "source": "manual", "id": "ENG-123" } }
        });

        let (status, _) = request_json(app, Method::POST, "/run/planner", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn bearer_token_is_enforced_on_protected_routes() {
        let mut config = test_config();
        config.bearer_token = Some("secret".to_owned());
        let app = build_router(config);
        let body = json!({
            "action_id": "01J9XYZ",
            "payload": base_payload()
        });

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/run/planner")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let ok = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/run/planner")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer secret")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn healthz_is_open_even_when_bearer_token_is_set() {
        let mut config = test_config();
        config.bearer_token = Some("secret".to_owned());
        let app = build_router(config);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invocations_requires_bearer_when_configured() {
        let mut config = test_config();
        config.bearer_token = Some("secret".to_owned());
        let app = build_router(config);
        let body = json!({
            "agent_type": "triage",
            "action_id": "01J9XYZ",
            "payload": base_payload()
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/invocations")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn agentcore_invocations_surface_dispatches() {
        let app = build_router(test_config());
        let body = json!({
            "agent_type": "triage",
            "action_id": "01J9XYZ",
            "payload": base_payload()
        });

        let (status, json) = request_json(app, Method::POST, "/invocations", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["output"]["accepted"], true);
    }

    #[tokio::test]
    async fn ping_is_healthy_without_bearer_token() {
        let mut config = test_config();
        config.bearer_token = Some("secret".to_owned());
        let app = build_router(config);
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
