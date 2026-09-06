use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use serde::{Deserialize, Serialize};

use crate::{app::AppState, auth::unix_timestamp, database::SettingsRow, public_snapshot};

use super::{
    auth::{ApiError, authenticate_session, json_response, parse_json, validate_csrf},
    patch::PatchField,
};

const JSON_BODY_LIMIT: usize = 8 * 1_024;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PatchSettingsRequest {
    #[serde(default)]
    site_name: PatchField<String>,
    #[serde(default)]
    site_timezone: PatchField<String>,
    #[serde(default)]
    theme_default: PatchField<String>,
    #[serde(default)]
    history_retention_days: PatchField<i64>,
    #[serde(default)]
    agent_report_interval_seconds: PatchField<i64>,
    #[serde(default)]
    ping_interval_seconds: PatchField<i64>,
    #[serde(default)]
    offline_after_seconds: PatchField<i64>,
    #[serde(default)]
    default_traffic_reset_day: PatchField<i64>,
}

#[derive(Default)]
struct SettingsPatch {
    site_name: Option<String>,
    site_timezone: Option<String>,
    theme_default: Option<String>,
    history_retention_days: Option<i64>,
    agent_report_interval_seconds: Option<i64>,
    ping_interval_seconds: Option<i64>,
    offline_after_seconds: Option<i64>,
    default_traffic_reset_day: Option<i64>,
}

#[derive(Serialize)]
struct SettingsResponse {
    site_name: String,
    site_timezone: String,
    theme_default: String,
    history_retention_days: i64,
    agent_report_interval_seconds: i64,
    ping_interval_seconds: i64,
    offline_after_seconds: i64,
    default_traffic_reset_day: i64,
}

impl From<SettingsRow> for SettingsResponse {
    fn from(settings: SettingsRow) -> Self {
        Self {
            site_name: settings.site_name,
            site_timezone: settings.site_timezone,
            theme_default: settings.theme_default,
            history_retention_days: settings.history_retention_days,
            agent_report_interval_seconds: settings.agent_report_interval_seconds,
            ping_interval_seconds: settings.ping_interval_seconds,
            offline_after_seconds: settings.offline_after_seconds,
            default_traffic_reset_day: settings.default_traffic_reset_day,
        }
    }
}

pub(super) async fn get(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    let settings = state
        .database
        .load_settings()
        .await
        .map_err(ApiError::database)?;
    Ok(json_response(
        StatusCode::OK,
        SettingsResponse::from(settings),
    ))
}

pub(super) async fn patch(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    let request: PatchSettingsRequest = parse_json(request, JSON_BODY_LIMIT).await?;
    let patch = validate_patch(request)?;

    let mutation_guard = state.settings_mutation_lock.lock().await;
    let mut settings = state.settings.read().await.clone();
    patch.apply(&mut settings);
    validate_merged(&settings)?;
    settings.updated_at = unix_timestamp().map_err(|_| ApiError::internal())?;
    state
        .commit_settings_under_lock(settings.clone())
        .await
        .map_err(ApiError::database)?;
    drop(mutation_guard);

    if let Err(error) = public_snapshot::generate_now(&state).await {
        tracing::warn!(error = %error, "immediate public snapshot refresh failed; previous cache retained");
    }
    Ok(json_response(
        StatusCode::OK,
        SettingsResponse::from(settings),
    ))
}

fn validate_patch(request: PatchSettingsRequest) -> Result<SettingsPatch, ApiError> {
    if request.is_empty() {
        return Err(ApiError::invalid_request());
    }
    Ok(SettingsPatch {
        site_name: required_patch(request.site_name, |value| bounded_string(value, 64))?,
        site_timezone: required_patch(request.site_timezone, valid_timezone)?,
        theme_default: required_patch(request.theme_default, valid_theme)?,
        history_retention_days: required_patch(request.history_retention_days, |value| {
            ranged(value, 1, 30)
        })?,
        agent_report_interval_seconds: required_patch(
            request.agent_report_interval_seconds,
            |value| ranged(value, 2, 60),
        )?,
        ping_interval_seconds: required_patch(request.ping_interval_seconds, |value| {
            ranged(value, 10, 300)
        })?,
        offline_after_seconds: required_patch(request.offline_after_seconds, |value| {
            ranged(value, 5, 600)
        })?,
        default_traffic_reset_day: required_patch(request.default_traffic_reset_day, |value| {
            ranged(value, 1, 31)
        })?,
    })
}

impl PatchSettingsRequest {
    fn is_empty(&self) -> bool {
        self.site_name.is_missing()
            && self.site_timezone.is_missing()
            && self.theme_default.is_missing()
            && self.history_retention_days.is_missing()
            && self.agent_report_interval_seconds.is_missing()
            && self.ping_interval_seconds.is_missing()
            && self.offline_after_seconds.is_missing()
            && self.default_traffic_reset_day.is_missing()
    }
}

impl SettingsPatch {
    fn apply(self, settings: &mut SettingsRow) {
        if let Some(value) = self.site_name {
            settings.site_name = value;
        }
        if let Some(value) = self.site_timezone {
            settings.site_timezone = value;
        }
        if let Some(value) = self.theme_default {
            settings.theme_default = value;
        }
        if let Some(value) = self.history_retention_days {
            settings.history_retention_days = value;
        }
        if let Some(value) = self.agent_report_interval_seconds {
            settings.agent_report_interval_seconds = value;
        }
        if let Some(value) = self.ping_interval_seconds {
            settings.ping_interval_seconds = value;
        }
        if let Some(value) = self.offline_after_seconds {
            settings.offline_after_seconds = value;
        }
        if let Some(value) = self.default_traffic_reset_day {
            settings.default_traffic_reset_day = value;
        }
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

fn bounded_string(value: String, max_chars: usize) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    (1..=max_chars)
        .contains(&value.chars().count())
        .then_some(value)
        .ok_or_else(ApiError::invalid_request)
}

fn valid_timezone(value: String) -> Result<String, ApiError> {
    let value = bounded_string(value, 64)?;
    jiff::tz::TimeZone::get(&value)
        .map(|_| value)
        .map_err(|_| ApiError::invalid_request())
}

fn valid_theme(value: String) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    matches!(value.as_str(), "light" | "dark" | "system")
        .then_some(value)
        .ok_or_else(ApiError::invalid_request)
}

fn ranged(value: i64, min: i64, max: i64) -> Result<i64, ApiError> {
    (min..=max)
        .contains(&value)
        .then_some(value)
        .ok_or_else(ApiError::invalid_request)
}

fn validate_merged(settings: &SettingsRow) -> Result<(), ApiError> {
    if settings.offline_after_seconds <= settings.agent_report_interval_seconds {
        return Err(ApiError::invalid_request());
    }
    Ok(())
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
        extract::Path,
        http::{Request as HttpRequest, header::CONTENT_TYPE},
        response::IntoResponse,
    };
    use rusqlite::Connection;
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        auth::{encode_hex, sha256},
        database::{Database, NewNodeRow, hydrate_startup},
        http::{agent, nodes, ping_targets},
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);
    const SESSION_RAW: [u8; 32] = [0x71; 32];
    const CSRF_RAW: [u8; 32] = [0x72; 32];
    const AGENT_RAW: [u8; 32] = [0x73; 32];

    struct TestContext {
        state: AppState,
        path: PathBuf,
        cookie: String,
        csrf: String,
        existing_node_id: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "monitor-settings-admin-{}-{id}.db",
                std::process::id()
            ));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open test database");
            let admin_hash = "a".repeat(32);
            database
                .set_admin_password(admin_hash.clone(), 1)
                .await
                .expect("set administrator");
            let now = unix_timestamp().expect("timestamp");
            assert!(
                database
                    .create_session(admin_hash, sha256(&SESSION_RAW), now, now + 3_600)
                    .await
                    .expect("create session")
            );
            let existing = database
                .create_node(
                    NewNodeRow {
                        public_id: "e".repeat(32),
                        name: "Existing".into(),
                        region_code: "US".into(),
                        traffic_limit_bytes: None,
                        traffic_reset_day: 1,
                        price_micros: None,
                        currency: None,
                        renewal_cycle: None,
                        expires_at: None,
                    },
                    sha256(&AGENT_RAW),
                    now,
                )
                .await
                .expect("create existing node");
            let hydration = hydrate_startup(&database).await.expect("hydrate state");
            let state = AppState::new(database, hydration);
            public_snapshot::generate(&state, now)
                .await
                .expect("initial public snapshot");
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
                existing_node_id: existing.id,
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
            .expect("body");
        serde_json::from_slice(&body).expect("JSON")
    }

    async fn patch_settings(context: &TestContext, body: &str) -> Response {
        response(
            patch(
                State(context.state.clone()),
                context.request(body, true, true),
            )
            .await,
        )
    }

    #[tokio::test]
    async fn get_returns_exact_settings_contract_and_requires_session() {
        let context = TestContext::new().await;
        assert_eq!(
            response(
                get(
                    State(context.state.clone()),
                    context.request("", false, false)
                )
                .await
            )
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = response(
            get(
                State(context.state.clone()),
                context.request("", true, false),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body.as_object().unwrap().len(), 8);
        assert_eq!(body["site_name"], "Monitor");
        assert_eq!(body["site_timezone"], "Asia/Shanghai");
        assert!(body.get("updated_at").is_none());
        context.finish().await;
    }

    #[tokio::test]
    async fn patch_validation_rejects_invalid_null_unknown_size_and_content_type() {
        let context = TestContext::new().await;
        let invalid = [
            json!({"site_name":"   "}).to_string(),
            json!({"site_name":"x".repeat(65)}).to_string(),
            json!({"site_timezone":"Not/A_Zone"}).to_string(),
            json!({"theme_default":"auto"}).to_string(),
            json!({"history_retention_days":0}).to_string(),
            json!({"history_retention_days":31}).to_string(),
            json!({"agent_report_interval_seconds":1}).to_string(),
            json!({"agent_report_interval_seconds":61}).to_string(),
            json!({"ping_interval_seconds":9}).to_string(),
            json!({"ping_interval_seconds":301}).to_string(),
            json!({"offline_after_seconds":4}).to_string(),
            json!({"offline_after_seconds":601}).to_string(),
            json!({"default_traffic_reset_day":0}).to_string(),
            json!({"default_traffic_reset_day":32}).to_string(),
            json!({"agent_report_interval_seconds":20}).to_string(),
            json!({"unknown":true}).to_string(),
            r#"{"site_name":null,"theme_default":"dark"}"#.to_owned(),
            "{}".to_owned(),
        ];
        for body in invalid {
            assert_eq!(
                patch_settings(&context, &body).await.status(),
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
        assert_eq!(context.state.settings.read().await.theme_default, "system");
        assert_eq!(
            patch_settings(
                &context,
                r#"{"agent_report_interval_seconds":20,"offline_after_seconds":21}"#,
            )
            .await
            .status(),
            StatusCode::OK
        );
        let oversized = json!({"site_name":"x".repeat(8_300)}).to_string();
        assert_eq!(
            patch_settings(&context, &oversized).await.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let mut wrong_type = context.request(r#"{"theme_default":"dark"}"#, true, true);
        wrong_type.headers_mut().remove(CONTENT_TYPE);
        assert_eq!(
            response(patch(State(context.state.clone()), wrong_type).await).status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn patch_updates_caches_agent_config_public_snapshot_and_default_for_new_nodes() {
        let context = TestContext::new().await;
        let traffic_before = context.state.traffic.read_current();
        let patch_response = patch_settings(
            &context,
            r#"{
                "site_name":" New Monitor ",
                "site_timezone":"America/New_York",
                "theme_default":"dark",
                "agent_report_interval_seconds":3,
                "ping_interval_seconds":20,
                "offline_after_seconds":12,
                "default_traffic_reset_day":15
            }"#,
        )
        .await;
        assert_eq!(patch_response.status(), StatusCode::OK);
        let body = json_body(patch_response).await;
        assert_eq!(body["site_name"], "New Monitor");
        assert_eq!(body["site_timezone"], "America/New_York");
        let config = context.state.agent_config.read().await;
        assert_eq!(config.report_interval_seconds, 3);
        assert_eq!(config.ping_interval_seconds, 20);
        drop(config);
        let cached = context.state.public_snapshot.load().await;
        let public: Value = serde_json::from_slice(&cached.body).unwrap();
        assert_eq!(public["site"]["name"], "New Monitor");
        assert_eq!(public["site"]["timezone"], "America/New_York");
        assert_eq!(public["site"]["theme_default"], "dark");
        assert_eq!(context.state.traffic.read_current(), traffic_before);

        let config_response = response(
            agent::config(
                State(context.state.clone()),
                HttpRequest::builder()
                    .header(
                        "Authorization",
                        format!("Bearer {}", encode_hex(&AGENT_RAW)),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        );
        assert_eq!(config_response.status(), StatusCode::OK);
        let config_json = json_body(config_response).await;
        assert_eq!(config_json["report_interval_seconds"], 3);
        assert_eq!(config_json["ping_interval_seconds"], 20);

        let node_response = response(
            nodes::create(
                State(context.state.clone()),
                context.request(r#"{"name":"New","region_code":"JP"}"#, true, true),
            )
            .await,
        );
        assert_eq!(node_response.status(), StatusCode::CREATED);
        assert_eq!(
            json_body(node_response).await["node"]["traffic_reset_day"],
            15
        );
        assert_eq!(
            context
                .state
                .node_metadata
                .read()
                .await
                .get(&context.existing_node_id)
                .unwrap()
                .traffic_reset_day,
            1
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn database_failure_leaves_all_settings_publications_unchanged() {
        let context = TestContext::new().await;
        let settings_before = context.state.settings.read().await.clone();
        let config_before = context.state.agent_config.read().await.clone();
        let public_before = context.state.public_snapshot.load().await;
        let connection = Connection::open(&context.path).expect("trigger connection");
        connection
            .execute_batch(
                "CREATE TRIGGER reject_settings BEFORE UPDATE ON settings
                 BEGIN SELECT RAISE(FAIL, 'test failure'); END;",
            )
            .expect("failure trigger");
        drop(connection);

        assert_eq!(
            patch_settings(&context, r#"{"site_name":"Rejected"}"#)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(*context.state.settings.read().await, settings_before);
        let config = context.state.agent_config.read().await;
        assert_eq!(
            config.report_interval_seconds,
            config_before.report_interval_seconds
        );
        assert_eq!(
            config.ping_interval_seconds,
            config_before.ping_interval_seconds
        );
        assert_eq!(config.targets, config_before.targets);
        drop(config);
        let public_after = context.state.public_snapshot.load().await;
        assert!(std::sync::Arc::ptr_eq(&public_before, &public_after));
        context.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_settings_and_ping_mutations_preserve_owned_agent_config_fields() {
        let context = TestContext::new().await;
        let mut settings_tasks = Vec::new();
        for index in 0..8 {
            let state = context.state.clone();
            let cookie = context.cookie.clone();
            let csrf = context.csrf.clone();
            settings_tasks.push(tokio::spawn(async move {
                let body = json!({
                    "site_name": format!("Concurrent {index}"),
                    "agent_report_interval_seconds": 2 + index % 3,
                    "offline_after_seconds": 20,
                    "ping_interval_seconds": 15 + index,
                })
                .to_string();
                response(patch(State(state), request(&cookie, &csrf, &body, true, true)).await)
                    .status()
            }));
        }
        let state = context.state.clone();
        let cookie = context.cookie.clone();
        let csrf = context.csrf.clone();
        let ping_task = tokio::spawn(async move {
            let body = r#"{"name":"Concurrent Target","host":"target.example","ip_family":4,"enabled":true}"#;
            response(
                ping_targets::create(State(state), request(&cookie, &csrf, body, true, true)).await,
            )
            .status()
        });
        for task in settings_tasks {
            assert_eq!(task.await.unwrap(), StatusCode::OK);
        }
        assert_eq!(ping_task.await.unwrap(), StatusCode::CREATED);

        let persisted = context.state.database.load_settings().await.unwrap();
        let cached_settings = context.state.settings.read().await.clone();
        assert_eq!(persisted, cached_settings);
        let persisted_targets = context
            .state
            .database
            .load_enabled_ping_targets()
            .await
            .unwrap();
        let config = context.state.agent_config.read().await;
        assert_eq!(
            config.report_interval_seconds,
            cached_settings.agent_report_interval_seconds
        );
        assert_eq!(
            config.ping_interval_seconds,
            cached_settings.ping_interval_seconds
        );
        assert_eq!(config.targets.len(), persisted_targets.len());
        assert!(
            config
                .targets
                .iter()
                .zip(&persisted_targets)
                .all(|(a, b)| a.id == b.id
                    && a.name == b.name
                    && a.host == b.host
                    && a.ip_family == b.ip_family)
        );
        drop(config);
        context.finish().await;
    }

    #[tokio::test]
    async fn malformed_ping_id_and_missing_csrf_are_rejected() {
        let context = TestContext::new().await;
        for id in ["0", "-1", "no", "999999999999999999999999"] {
            assert_eq!(
                response(
                    ping_targets::patch(
                        State(context.state.clone()),
                        Path(id.to_owned()),
                        context.request(r#"{"enabled":false}"#, true, true),
                    )
                    .await,
                )
                .status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            response(
                patch(
                    State(context.state.clone()),
                    context.request(r#"{"site_name":"No CSRF"}"#, true, false),
                )
                .await,
            )
            .status(),
            StatusCode::FORBIDDEN
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
