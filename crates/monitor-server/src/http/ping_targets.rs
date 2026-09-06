use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::Response,
};
use serde::{Deserialize, Serialize};

use crate::{
    app::{AgentPingTarget, AppState},
    auth::unix_timestamp,
    database::{
        CreatePingTargetResult, EnabledPingTargetRow, NewPingTargetRow, PingTargetPatchRow,
        PingTargetRow, UpdatePingTargetResult,
    },
    ping_target::valid_host_for_family,
};

use super::{
    auth::{
        ApiError, authenticate_session, json_response, no_content_response, parse_json,
        validate_csrf,
    },
    patch::PatchField,
};

const JSON_BODY_LIMIT: usize = 8 * 1_024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePingTargetRequest {
    name: String,
    host: String,
    ip_family: i64,
    enabled: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PatchPingTargetRequest {
    #[serde(default)]
    name: PatchField<String>,
    #[serde(default)]
    host: PatchField<String>,
    #[serde(default)]
    ip_family: PatchField<i64>,
    #[serde(default)]
    enabled: PatchField<bool>,
    #[serde(default)]
    sort_order: PatchField<i64>,
}

#[derive(Serialize)]
struct PingTargetsResponse {
    targets: Vec<PingTargetObject>,
}

#[derive(Serialize)]
struct PingTargetResponse {
    target: PingTargetObject,
}

#[derive(Serialize)]
struct PingTargetObject {
    id: i64,
    name: String,
    host: String,
    ip_family: i64,
    enabled: bool,
    sort_order: i64,
}

impl From<PingTargetRow> for PingTargetObject {
    fn from(target: PingTargetRow) -> Self {
        Self {
            id: target.id,
            name: target.name,
            host: target.host,
            ip_family: target.ip_family,
            enabled: target.enabled,
            sort_order: target.sort_order,
        }
    }
}

pub(super) async fn list(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    let targets = state
        .database
        .list_ping_targets()
        .await
        .map_err(ApiError::database)?
        .into_iter()
        .map(PingTargetObject::from)
        .collect();
    Ok(json_response(
        StatusCode::OK,
        PingTargetsResponse { targets },
    ))
}

pub(super) async fn create(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    let request: CreatePingTargetRequest = parse_json(request, JSON_BODY_LIMIT).await?;
    let target = NewPingTargetRow {
        name: valid_name(request.name)?,
        host: valid_host(request.host, request.ip_family)?,
        ip_family: valid_ip_family(request.ip_family)?,
        enabled: request.enabled,
    };

    let _mutation_guard = state.ping_target_mutation_lock.lock().await;
    let result = state
        .database
        .create_ping_target(target, unix_timestamp().map_err(|_| ApiError::internal())?)
        .await
        .map_err(ApiError::database)?;
    let result = match result {
        CreatePingTargetResult::Conflict => return Err(ApiError::conflict()),
        CreatePingTargetResult::Created(result) => result,
    };
    publish_enabled_targets(&state, result.enabled_targets).await;
    Ok(json_response(
        StatusCode::CREATED,
        PingTargetResponse {
            target: PingTargetObject::from(result.target),
        },
    ))
}

pub(super) async fn patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    let id = valid_id(&id)?;
    let request: PatchPingTargetRequest = parse_json(request, JSON_BODY_LIMIT).await?;
    let patch = validate_patch(request)?;

    let _mutation_guard = state.ping_target_mutation_lock.lock().await;
    let result = state
        .database
        .update_ping_target(
            id,
            patch,
            unix_timestamp().map_err(|_| ApiError::internal())?,
        )
        .await
        .map_err(ApiError::database)?;
    let result = match result {
        UpdatePingTargetResult::NotFound => return Err(ApiError::not_found()),
        UpdatePingTargetResult::InvalidSortOrder | UpdatePingTargetResult::InvalidConfiguration => {
            return Err(ApiError::invalid_request());
        }
        UpdatePingTargetResult::Conflict => return Err(ApiError::conflict()),
        UpdatePingTargetResult::Updated(result) => result,
    };
    publish_enabled_targets(&state, result.enabled_targets).await;
    Ok(json_response(
        StatusCode::OK,
        PingTargetResponse {
            target: PingTargetObject::from(result.target),
        },
    ))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    let id = valid_id(&id)?;

    let _mutation_guard = state.ping_target_mutation_lock.lock().await;
    let deleted = state
        .database
        .delete_ping_target(id, unix_timestamp().map_err(|_| ApiError::internal())?)
        .await
        .map_err(ApiError::database)?
        .ok_or_else(ApiError::not_found)?;
    publish_enabled_targets(&state, deleted.enabled_targets).await;
    state.history.remove_target(deleted.target_id);
    Ok(no_content_response())
}

fn validate_patch(request: PatchPingTargetRequest) -> Result<PingTargetPatchRow, ApiError> {
    if request.is_empty() {
        return Err(ApiError::invalid_request());
    }
    Ok(PingTargetPatchRow {
        name: required_patch(request.name, valid_name)?,
        host: required_patch(request.host, valid_host_syntax)?,
        ip_family: required_patch(request.ip_family, valid_ip_family)?,
        enabled: required_patch(request.enabled, Ok)?,
        sort_order: required_patch(request.sort_order, |value| {
            (value >= 0)
                .then_some(value)
                .ok_or_else(ApiError::invalid_request)
        })?,
    })
}

impl PatchPingTargetRequest {
    fn is_empty(&self) -> bool {
        self.name.is_missing()
            && self.host.is_missing()
            && self.ip_family.is_missing()
            && self.enabled.is_missing()
            && self.sort_order.is_missing()
    }
}

fn required_patch<T, U>(
    field: PatchField<T>,
    validate: impl FnOnce(T) -> Result<U, ApiError>,
) -> Result<Option<U>, ApiError> {
    match field {
        PatchField::Missing => Ok(None),
        PatchField::Null => Err(ApiError::invalid_request()),
        PatchField::Value(value) => validate(value).map(Some),
    }
}

fn valid_name(value: String) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    (1..=64)
        .contains(&value.chars().count())
        .then_some(value)
        .ok_or_else(ApiError::invalid_request)
}

fn valid_host(value: String, family: i64) -> Result<String, ApiError> {
    let value = valid_host_syntax(value)?;
    valid_host_for_family(&value, family)
        .then_some(value)
        .ok_or_else(ApiError::invalid_request)
}

fn valid_host_syntax(value: String) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed != value || !valid_host_for_family(trimmed, 4) && !valid_host_for_family(trimmed, 6)
    {
        return Err(ApiError::invalid_request());
    }
    Ok(trimmed.to_owned())
}

fn valid_ip_family(value: i64) -> Result<i64, ApiError> {
    matches!(value, 4 | 6)
        .then_some(value)
        .ok_or_else(ApiError::invalid_request)
}

fn valid_id(value: &str) -> Result<i64, ApiError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::not_found());
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(ApiError::not_found)
}

async fn publish_enabled_targets(state: &AppState, rows: Vec<EnabledPingTargetRow>) {
    state.agent_config.write().await.targets = rows
        .into_iter()
        .map(|target| AgentPingTarget {
            id: target.id,
            name: target.name,
            host: target.host,
            ip_family: target.ip_family,
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path as FsPath, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        body::{Body, to_bytes},
        http::{Request as HttpRequest, header::CONTENT_TYPE},
        response::IntoResponse,
    };
    use rusqlite::{Connection, params};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        auth::{encode_hex, sha256},
        database::{Database, NewNodeRow, hydrate_startup},
        history::{PingSample, ResourceSample},
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);
    const SESSION_RAW: [u8; 32] = [0x61; 32];
    const CSRF_RAW: [u8; 32] = [0x62; 32];

    struct TestContext {
        state: AppState,
        path: PathBuf,
        cookie: String,
        csrf: String,
    }

    impl TestContext {
        async fn new() -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("monitor-ping-admin-{}-{id}.db", std::process::id()));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open test database");
            let admin_hash = "a".repeat(32);
            database
                .set_admin_password(admin_hash.clone(), 1)
                .await
                .expect("set administrator fixture");
            let now = unix_timestamp().expect("timestamp");
            assert!(
                database
                    .create_session(admin_hash, sha256(&SESSION_RAW), now, now + 3_600)
                    .await
                    .expect("create session")
            );
            let hydration = hydrate_startup(&database).await.expect("hydrate state");
            let state = AppState::new(database, hydration);
            let csrf = encode_hex(&CSRF_RAW);
            let cookie = format!(
                "__Host-monitor_session={}; __Host-monitor_csrf={csrf}",
                encode_hex(&SESSION_RAW)
            );
            Self {
                state,
                path,
                cookie,
                csrf,
            }
        }

        fn request(&self, body: &str, authenticated: bool, csrf: bool) -> Request {
            request(&self.cookie, &self.csrf, body, authenticated, csrf)
        }

        async fn finish(self) {
            self.state
                .database
                .shutdown()
                .await
                .expect("shutdown database");
            remove_database_files(&self.path);
        }
    }

    fn request(cookie: &str, csrf_token: &str, body: &str, auth: bool, csrf: bool) -> Request {
        let mut builder = HttpRequest::builder();
        if auth {
            builder = builder.header("Cookie", cookie);
        }
        if csrf {
            builder = builder
                .header("Host", "monitor.test")
                .header("Origin", "https://monitor.test")
                .header("X-CSRF-Token", csrf_token);
        }
        builder
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request")
    }

    fn response(result: Result<Response, ApiError>) -> Response {
        result.unwrap_or_else(IntoResponse::into_response)
    }

    async fn json_body(response: Response) -> Value {
        let body = to_bytes(response.into_body(), 64 * 1_024)
            .await
            .expect("response body");
        serde_json::from_slice(&body).expect("response JSON")
    }

    async fn create_target(
        context: &TestContext,
        name: &str,
        host: &str,
        family: i64,
        enabled: bool,
    ) -> (StatusCode, Value) {
        let body = json!({
            "name": name,
            "host": host,
            "ip_family": family,
            "enabled": enabled,
        })
        .to_string();
        let response = response(
            create(
                State(context.state.clone()),
                context.request(&body, true, true),
            )
            .await,
        );
        let status = response.status();
        (status, json_body(response).await)
    }

    #[tokio::test]
    async fn get_and_create_cover_auth_sorting_literals_dns_and_cache_publication() {
        let context = TestContext::new().await;
        assert_eq!(
            response(
                list(
                    State(context.state.clone()),
                    context.request("", false, false)
                )
                .await
            )
            .status(),
            StatusCode::UNAUTHORIZED
        );
        for (name, host, family, enabled) in [
            ("DNS", "Example.COM", 4, true),
            ("IPv4", "203.0.113.1", 4, false),
            ("IPv6", "2001:db8::1", 6, true),
        ] {
            assert_eq!(
                create_target(&context, name, host, family, enabled).await.0,
                StatusCode::CREATED
            );
        }
        let response = response(
            list(
                State(context.state.clone()),
                context.request("", true, false),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let targets = body["targets"].as_array().expect("targets");
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0]["sort_order"], 0);
        assert_eq!(targets[1]["sort_order"], 1);
        assert_eq!(targets[2]["sort_order"], 2);
        assert_eq!(targets[0]["host"], "Example.COM");
        let config = context.state.agent_config.read().await;
        assert_eq!(config.targets.len(), 2);
        assert_eq!(config.targets[0].name, "DNS");
        assert_eq!(config.targets[1].name, "IPv6");
        drop(config);
        context.finish().await;
    }

    #[tokio::test]
    async fn request_and_host_validation_are_strict() {
        let context = TestContext::new().await;
        let valid_body = r#"{"name":"Target","host":"example.com","ip_family":4,"enabled":true}"#;
        assert_eq!(
            response(
                create(
                    State(context.state.clone()),
                    context.request(valid_body, true, false),
                )
                .await,
            )
            .status(),
            StatusCode::FORBIDDEN
        );
        let invalid = [
            ("https://example.com", 4),
            ("example.com:443", 4),
            ("example.com/path", 4),
            ("user@example.com", 4),
            ("example.com ", 4),
            ("bad_name.example", 4),
            ("abc..def", 4),
            ("$(id)", 4),
            (";reboot", 4),
            ("203.0.113.1", 6),
            ("2001:db8::1", 4),
        ];
        for (host, family) in invalid {
            assert_eq!(
                create_target(&context, "Target", host, family, true)
                    .await
                    .0,
                StatusCode::BAD_REQUEST,
                "{host}"
            );
        }
        for host in [format!("{}.com", "a".repeat(64)), "a".repeat(254)] {
            assert_eq!(
                create_target(&context, "Target", &host, 4, true).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        for body in [
            r#"{"name":"Target","host":"example.com","ip_family":4,"enabled":true,"extra":1}"#,
            r#"{"name":"Target","host":"example.com","ip_family":"4","enabled":true}"#,
        ] {
            assert_eq!(
                response(
                    create(
                        State(context.state.clone()),
                        context.request(body, true, true)
                    )
                    .await
                )
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let oversized = json!({
            "name": "x".repeat(8_300), "host": "example.com", "ip_family": 4, "enabled": true
        })
        .to_string();
        assert_eq!(
            response(
                create(
                    State(context.state.clone()),
                    context.request(&oversized, true, true)
                )
                .await
            )
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let mut wrong_type = context.request(
            r#"{"name":"T","host":"example.com","ip_family":4,"enabled":true}"#,
            true,
            true,
        );
        wrong_type.headers_mut().remove(CONTENT_TYPE);
        assert_eq!(
            response(create(State(context.state.clone()), wrong_type).await).status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn enabled_cap_rolls_back_database_and_cache() {
        let context = TestContext::new().await;
        for index in 0..6 {
            assert_eq!(
                create_target(
                    &context,
                    &format!("T{index}"),
                    &format!("t{index}.example"),
                    4,
                    true,
                )
                .await
                .0,
                StatusCode::CREATED
            );
        }
        let before = context.state.agent_config.read().await.targets.clone();
        let (status, body) = create_target(&context, "Seventh", "seven.example", 4, true).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "conflict");
        assert_eq!(context.state.agent_config.read().await.targets, before);
        assert_eq!(
            context
                .state
                .database
                .list_ping_targets()
                .await
                .unwrap()
                .len(),
            6
        );
        let (_, disabled) = create_target(&context, "Disabled", "disabled.example", 4, false).await;
        let disabled_id = disabled["target"]["id"].as_i64().unwrap();
        let before = context.state.agent_config.read().await.targets.clone();
        let enable = response(
            patch(
                State(context.state.clone()),
                Path(disabled_id.to_string()),
                context.request(r#"{"enabled":true}"#, true, true),
            )
            .await,
        );
        assert_eq!(enable.status(), StatusCode::CONFLICT);
        assert_eq!(context.state.agent_config.read().await.targets, before);
        assert!(
            !context
                .state
                .database
                .list_ping_targets()
                .await
                .unwrap()
                .into_iter()
                .find(|target| target.id == disabled_id)
                .unwrap()
                .enabled
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn patch_reorders_validates_final_pair_and_preserves_history_on_disable() {
        let context = TestContext::new().await;
        let mut ids = Vec::new();
        for index in 0..4 {
            let (_, body) = create_target(
                &context,
                &format!("T{index}"),
                &format!("t{index}.example"),
                4,
                true,
            )
            .await;
            ids.push(body["target"]["id"].as_i64().unwrap());
        }
        let patch_request = |id: i64, body: &str| {
            patch(
                State(context.state.clone()),
                Path(id.to_string()),
                context.request(body, true, true),
            )
        };
        assert_eq!(
            response(patch_request(ids[3], r#"{"sort_order":0}"#).await).status(),
            StatusCode::OK
        );
        assert_eq!(
            response(patch_request(ids[3], r#"{"sort_order":3}"#).await).status(),
            StatusCode::OK
        );
        assert_eq!(
            context
                .state
                .database
                .list_ping_targets()
                .await
                .unwrap()
                .iter()
                .map(|target| target.sort_order)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(
            response(patch_request(ids[0], r#"{"name":null,"enabled":false}"#).await).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            response(patch_request(ids[0], "{}").await).status(),
            StatusCode::BAD_REQUEST
        );
        let (_, literal) = create_target(&context, "Literal", "203.0.113.1", 4, false).await;
        let literal_id = literal["target"]["id"].as_i64().unwrap();
        assert_eq!(
            response(patch_request(literal_id, r#"{"ip_family":6}"#).await).status(),
            StatusCode::BAD_REQUEST
        );
        let edited = response(
            patch_request(
                ids[0],
                r#"{"name":"Renamed","host":"ipv6.example","ip_family":6}"#,
            )
            .await,
        );
        assert_eq!(edited.status(), StatusCode::OK);
        let edited = json_body(edited).await;
        assert_eq!(edited["target"]["name"], "Renamed");
        assert_eq!(edited["target"]["host"], "ipv6.example");
        assert_eq!(edited["target"]["ip_family"], 6);
        assert!(
            context
                .state
                .agent_config
                .read()
                .await
                .targets
                .iter()
                .any(|target| {
                    target.id == ids[0]
                        && target.name == "Renamed"
                        && target.host == "ipv6.example"
                        && target.ip_family == 6
                })
        );

        let node = context
            .state
            .database
            .create_node(
                NewNodeRow {
                    public_id: "d".repeat(32),
                    name: "Node".into(),
                    region_code: "US".into(),
                    traffic_limit_bytes: None,
                    traffic_reset_day: 1,
                    price_micros: None,
                    currency: None,
                    renewal_cycle: None,
                    expires_at: None,
                },
                [9; 32],
                1,
            )
            .await
            .expect("create history node");
        let connection = Connection::open(&context.path).expect("fixture connection");
        connection
            .execute(
                "INSERT INTO ping_history VALUES (?1, 60, ?2, 1, 0, NULL, NULL, NULL)",
                params![node.id, ids[0]],
            )
            .expect("ping history fixture");
        drop(connection);
        assert_eq!(
            response(patch_request(ids[0], r#"{"enabled":false}"#).await).status(),
            StatusCode::OK
        );
        let connection = Connection::open(&context.path).expect("inspect history");
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM ping_history WHERE target_id = ?1",
                [ids[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        drop(connection);
        assert!(
            !context
                .state
                .agent_config
                .read()
                .await
                .targets
                .iter()
                .any(|t| t.id == ids[0])
        );
        assert_eq!(
            response(patch_request(ids[0], r#"{"enabled":true}"#).await).status(),
            StatusCode::OK
        );
        assert!(
            context
                .state
                .agent_config
                .read()
                .await
                .targets
                .iter()
                .any(|t| t.id == ids[0])
        );
        context.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn delete_cascades_history_clears_accumulator_and_concurrent_updates_converge() {
        let context = TestContext::new().await;
        let (_, first) = create_target(&context, "First", "first.example", 4, true).await;
        let (_, second) = create_target(&context, "Second", "second.example", 4, true).await;
        let first_id = first["target"]["id"].as_i64().unwrap();
        let second_id = second["target"]["id"].as_i64().unwrap();
        let mut tasks = Vec::new();
        for index in 0..12 {
            let state = context.state.clone();
            let cookie = context.cookie.clone();
            let csrf = context.csrf.clone();
            tasks.push(tokio::spawn(async move {
                let body = json!({"name": format!("Second {index}")}).to_string();
                response(
                    patch(
                        State(state),
                        Path(second_id.to_string()),
                        request(&cookie, &csrf, &body, true, true),
                    )
                    .await,
                )
                .status()
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), StatusCode::OK);
        }
        let persisted_enabled = context
            .state
            .database
            .load_enabled_ping_targets()
            .await
            .unwrap();
        let cached = context.state.agent_config.read().await.targets.clone();
        assert_eq!(cached.len(), persisted_enabled.len());
        assert!(
            cached
                .iter()
                .zip(&persisted_enabled)
                .all(|(a, b)| a.id == b.id
                    && a.name == b.name
                    && a.host == b.host
                    && a.ip_family == b.ip_family)
        );

        let node = context
            .state
            .database
            .create_node(
                NewNodeRow {
                    public_id: "f".repeat(32),
                    name: "History node".into(),
                    region_code: "US".into(),
                    traffic_limit_bytes: None,
                    traffic_reset_day: 1,
                    price_micros: None,
                    currency: None,
                    renewal_cycle: None,
                    expires_at: None,
                },
                [8; 32],
                1,
            )
            .await
            .unwrap();
        let connection = Connection::open(&context.path).unwrap();
        connection
            .execute(
                "INSERT INTO ping_history VALUES (?1, 60, ?2, 1, 0, NULL, NULL, NULL)",
                params![node.id, first_id],
            )
            .unwrap();
        drop(connection);

        context.state.history.record(
            1,
            60,
            ResourceSample {
                cpu_usage: 0.0,
                load_1: 0.0,
                load_5: 0.0,
                load_15: 0.0,
                memory_used_bytes: 0,
                swap_used_bytes: 0,
                disk_used_bytes: 0,
                rx_rate_bytes_per_sec: 0,
                tx_rate_bytes_per_sec: 0,
            },
            [PingSample {
                target_id: first_id,
                success: false,
                latency_ms: None,
            }],
        );
        context.state.history.finalize_before(120);
        assert_eq!(context.state.history.ping_samples(first_id), 1);
        let delete_response = response(
            delete(
                State(context.state.clone()),
                Path(first_id.to_string()),
                context.request("", true, true),
            )
            .await,
        );
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(context.state.history.ping_samples(first_id), 0);
        let connection = Connection::open(&context.path).unwrap();
        let history_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM ping_history WHERE target_id = ?1",
                [first_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(history_count, 0);
        drop(connection);
        let targets = context.state.database.list_ping_targets().await.unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, second_id);
        assert_eq!(targets[0].sort_order, 0);
        assert!(
            context
                .state
                .agent_config
                .read()
                .await
                .targets
                .iter()
                .all(|t| t.id != first_id)
        );
        context.finish().await;
    }

    fn remove_database_files(path: &FsPath) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove test database: {error}"),
            }
        }
    }
}
