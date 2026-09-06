use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::Response,
};
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    app::AppState,
    auth::{encode_hex, random_token, sha256, unix_timestamp},
    database::{AdminNodeRow, NewNodeRow, NodeMetaRow, NodePatchRow, UpdateNodeResult},
};

use super::auth::{
    ApiError, authenticate_session, json_response, no_content_response, parse_json, validate_csrf,
};

const JSON_BODY_LIMIT: usize = 8 * 1_024;
const JS_SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateNodeRequest {
    name: String,
    region_code: String,
    #[serde(default)]
    traffic_limit: Option<i64>,
    #[serde(default)]
    traffic_reset_day: PatchField<i64>,
    #[serde(default)]
    price_micros: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    renewal_cycle: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PatchNodeRequest {
    #[serde(default)]
    name: PatchField<String>,
    #[serde(default)]
    region_code: PatchField<String>,
    #[serde(default)]
    traffic_limit: PatchField<i64>,
    #[serde(default)]
    traffic_reset_day: PatchField<i64>,
    #[serde(default)]
    price_micros: PatchField<i64>,
    #[serde(default)]
    currency: PatchField<String>,
    #[serde(default)]
    renewal_cycle: PatchField<String>,
    #[serde(default)]
    expires_at: PatchField<i64>,
    #[serde(default)]
    sort_order: PatchField<i64>,
}

#[derive(Default)]
enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for PatchField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Null, Self::Value))
    }
}

impl<T> PatchField<T> {
    const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[derive(Serialize)]
struct NodeConfigResponse {
    id: String,
    name: String,
    region_code: String,
    sort_order: i64,
    traffic_limit: Option<i64>,
    traffic_reset_day: i64,
    price_micros: Option<i64>,
    currency: Option<String>,
    renewal_cycle: Option<String>,
    expires_at: Option<i64>,
}

impl From<&NodeMetaRow> for NodeConfigResponse {
    fn from(node: &NodeMetaRow) -> Self {
        Self {
            id: node.public_id.clone(),
            name: node.name.clone(),
            region_code: node.region_code.clone(),
            sort_order: node.sort_order,
            traffic_limit: node.traffic_limit_bytes,
            traffic_reset_day: node.traffic_reset_day,
            price_micros: node.price_micros,
            currency: node.currency.clone(),
            renewal_cycle: node.renewal_cycle.clone(),
            expires_at: node.expires_at,
        }
    }
}

#[derive(Serialize)]
struct CreateNodeResponse {
    node: NodeConfigResponse,
    agent_token: String,
}

#[derive(Serialize)]
struct RotateTokenResponse {
    agent_token: String,
}

#[derive(Serialize)]
struct AdminNodesResponse {
    nodes: Vec<AdminNodeResponse>,
}

#[derive(Serialize)]
struct AdminNodeResponse {
    id: String,
    name: String,
    region_code: String,
    sort_order: i64,
    last_ip: Option<String>,
    online: bool,
    last_seen_at: Option<i64>,
    cycle_rx: i64,
    cycle_tx: i64,
    traffic_limit: Option<i64>,
    price_micros: Option<i64>,
    currency: Option<String>,
    renewal_cycle: Option<String>,
    expires_at: Option<i64>,
}

pub(super) async fn list(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    let settings = state.settings.read().await;
    let timezone = settings.site_timezone.clone();
    let offline_after_seconds = settings.offline_after_seconds;
    drop(settings);
    let now = unix_timestamp().map_err(|_| ApiError::internal())?;
    let day_start = day_start_utc(now, &timezone)?;
    let nodes = state
        .database
        .list_admin_nodes(day_start, now)
        .await
        .map_err(ApiError::database)?
        .into_iter()
        .map(|row| admin_node_response(row, now, offline_after_seconds))
        .collect();
    Ok(json_response(StatusCode::OK, AdminNodesResponse { nodes }))
}

pub(super) async fn create(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    let request: CreateNodeRequest = parse_json(request, JSON_BODY_LIMIT).await?;
    let default_reset_day = state.settings.read().await.default_traffic_reset_day;
    let node = validate_create(request, default_reset_day)?;
    let public_id = random_public_id().map_err(|_| ApiError::internal())?;
    let token = random_token().map_err(|_| ApiError::internal())?;
    let token_hash = sha256(&token);
    let now = unix_timestamp().map_err(|_| ApiError::internal())?;
    let new_node = NewNodeRow { public_id, ..node };

    let _mutation_guard = state.node_mutation_lock.lock().await;
    let created = state
        .database
        .create_node(new_node, token_hash, now)
        .await
        .map_err(ApiError::database)?;
    state
        .node_metadata
        .write()
        .await
        .insert(created.id, created.clone());
    state
        .node_tokens
        .write()
        .await
        .insert(token_hash, created.id);

    Ok(json_response(
        StatusCode::CREATED,
        CreateNodeResponse {
            node: NodeConfigResponse::from(&created),
            agent_token: encode_hex(&token),
        },
    ))
}

pub(super) async fn patch(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    if !valid_public_id(&public_id) {
        return Err(ApiError::not_found());
    }
    let request: PatchNodeRequest = parse_json(request, JSON_BODY_LIMIT).await?;
    let patch = validate_patch(request)?;
    let _mutation_guard = state.node_mutation_lock.lock().await;
    let result = state
        .database
        .update_node(
            public_id,
            patch,
            unix_timestamp().map_err(|_| ApiError::internal())?,
        )
        .await
        .map_err(ApiError::database)?;
    let result = match result {
        UpdateNodeResult::NotFound => return Err(ApiError::not_found()),
        UpdateNodeResult::InvalidSortOrder | UpdateNodeResult::InvalidConfiguration => {
            return Err(ApiError::invalid_request());
        }
        UpdateNodeResult::Updated(result) => result,
    };
    let mut cache = state.node_metadata.write().await;
    for (node_id, sort_order) in &result.reordered_nodes {
        if let Some(node) = cache.get_mut(node_id) {
            node.sort_order = *sort_order;
        }
    }
    cache.insert(result.node.id, result.node.clone());
    drop(cache);
    Ok(json_response(
        StatusCode::OK,
        NodeConfigResponse::from(&result.node),
    ))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    if !valid_public_id(&public_id) {
        return Err(ApiError::not_found());
    }
    let _mutation_guard = state.node_mutation_lock.lock().await;
    let deleted = state
        .database
        .delete_node(
            public_id,
            unix_timestamp().map_err(|_| ApiError::internal())?,
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(ApiError::not_found)?;
    state.node_tokens.write().await.remove(&deleted.token_hash);
    let mut metadata = state.node_metadata.write().await;
    metadata.remove(&deleted.node_id);
    for (node_id, sort_order) in deleted.reordered_nodes {
        if let Some(node) = metadata.get_mut(&node_id) {
            node.sort_order = sort_order;
        }
    }
    drop(metadata);
    Ok(no_content_response())
}

pub(super) async fn rotate_token(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    if !valid_public_id(&public_id) {
        return Err(ApiError::not_found());
    }
    let token = random_token().map_err(|_| ApiError::internal())?;
    let new_hash = sha256(&token);
    let _mutation_guard = state.node_mutation_lock.lock().await;
    let rotated = state
        .database
        .rotate_node_token(
            public_id,
            new_hash,
            unix_timestamp().map_err(|_| ApiError::internal())?,
        )
        .await
        .map_err(ApiError::database)?
        .ok_or_else(ApiError::not_found)?;
    let mut tokens = state.node_tokens.write().await;
    tokens.remove(&rotated.old_token_hash);
    tokens.insert(new_hash, rotated.node_id);
    drop(tokens);
    Ok(json_response(
        StatusCode::OK,
        RotateTokenResponse {
            agent_token: encode_hex(&token),
        },
    ))
}

fn validate_create(
    request: CreateNodeRequest,
    default_reset_day: i64,
) -> Result<NewNodeRow, ApiError> {
    let name = trimmed_name(request.name)?;
    let region_code = trimmed_region(request.region_code)?;
    validate_optional_positive(request.traffic_limit)?;
    let traffic_reset_day = match request.traffic_reset_day {
        PatchField::Missing => default_reset_day,
        PatchField::Null => return Err(ApiError::invalid_request()),
        PatchField::Value(value) => valid_reset_day(value)?,
    };
    validate_optional_nonnegative(request.price_micros)?;
    let currency = request.currency.map(trimmed_currency).transpose()?;
    if request.price_micros.is_some() != currency.is_some() {
        return Err(ApiError::invalid_request());
    }
    let renewal_cycle = request
        .renewal_cycle
        .map(trimmed_renewal_cycle)
        .transpose()?;
    validate_optional_nonnegative(request.expires_at)?;
    Ok(NewNodeRow {
        public_id: String::new(),
        name,
        region_code,
        traffic_limit_bytes: request.traffic_limit,
        traffic_reset_day,
        price_micros: request.price_micros,
        currency,
        renewal_cycle,
        expires_at: request.expires_at,
    })
}

fn validate_patch(request: PatchNodeRequest) -> Result<NodePatchRow, ApiError> {
    if request.is_empty() {
        return Err(ApiError::invalid_request());
    }
    Ok(NodePatchRow {
        name: required_patch(request.name, trimmed_name)?,
        region_code: required_patch(request.region_code, trimmed_region)?,
        traffic_limit_bytes: nullable_patch(request.traffic_limit, valid_positive)?,
        traffic_reset_day: required_patch(request.traffic_reset_day, valid_reset_day)?,
        price_micros: nullable_patch(request.price_micros, valid_nonnegative)?,
        currency: nullable_patch(request.currency, trimmed_currency)?,
        renewal_cycle: nullable_patch(request.renewal_cycle, trimmed_renewal_cycle)?,
        expires_at: nullable_patch(request.expires_at, valid_nonnegative)?,
        sort_order: required_patch(request.sort_order, valid_nonnegative)?,
    })
}

impl PatchNodeRequest {
    fn is_empty(&self) -> bool {
        self.name.is_missing()
            && self.region_code.is_missing()
            && self.traffic_limit.is_missing()
            && self.traffic_reset_day.is_missing()
            && self.price_micros.is_missing()
            && self.currency.is_missing()
            && self.renewal_cycle.is_missing()
            && self.expires_at.is_missing()
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

fn nullable_patch<T, U>(
    field: PatchField<T>,
    validate: impl FnOnce(T) -> Result<U, ApiError>,
) -> Result<Option<Option<U>>, ApiError> {
    match field {
        PatchField::Missing => Ok(None),
        PatchField::Null => Ok(Some(None)),
        PatchField::Value(value) => validate(value).map(|value| Some(Some(value))),
    }
}

fn trimmed_name(value: String) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    if (1..=64).contains(&value.chars().count()) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn trimmed_region(value: String) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    if uppercase_ascii_code(&value, 2) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn trimmed_currency(value: String) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    if uppercase_ascii_code(&value, 3) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn trimmed_renewal_cycle(value: String) -> Result<String, ApiError> {
    let value = value.trim().to_owned();
    if matches!(
        value.as_str(),
        "monthly" | "quarterly" | "semiannual" | "annual" | "biennial" | "custom"
    ) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn uppercase_ascii_code(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn validate_optional_positive(value: Option<i64>) -> Result<(), ApiError> {
    value.map(valid_positive).transpose().map(drop)
}

fn validate_optional_nonnegative(value: Option<i64>) -> Result<(), ApiError> {
    value.map(valid_nonnegative).transpose().map(drop)
}

fn valid_positive(value: i64) -> Result<i64, ApiError> {
    if (1..=JS_SAFE_INTEGER_MAX).contains(&value) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn valid_nonnegative(value: i64) -> Result<i64, ApiError> {
    if (0..=JS_SAFE_INTEGER_MAX).contains(&value) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn valid_reset_day(value: i64) -> Result<i64, ApiError> {
    if (1..=31).contains(&value) {
        Ok(value)
    } else {
        Err(ApiError::invalid_request())
    }
}

fn random_public_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = [0_u8; 32];
    for (index, byte) in bytes.iter().copied().enumerate() {
        output[index * 2] = DIGITS[usize::from(byte >> 4)];
        output[index * 2 + 1] = DIGITS[usize::from(byte & 0x0f)];
    }
    Ok(String::from_utf8(output.to_vec()).expect("lowercase hexadecimal is valid UTF-8"))
}

fn valid_public_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn day_start_utc(now: i64, timezone: &str) -> Result<i64, ApiError> {
    let timestamp = Timestamp::from_second(now).map_err(|error| {
        tracing::error!(error = %error, "current timestamp is outside jiff range");
        ApiError::internal()
    })?;
    let zoned = timestamp.in_tz(timezone).map_err(|error| {
        tracing::error!(timezone, error = %error, "site timezone is invalid");
        ApiError::internal()
    })?;
    zoned
        .start_of_day()
        .map(|start| start.timestamp().as_second())
        .map_err(|error| {
            tracing::error!(timezone, error = %error, "site day start is outside jiff range");
            ApiError::internal()
        })
}

fn admin_node_response(row: AdminNodeRow, now: i64, offline_after: i64) -> AdminNodeResponse {
    let online = row
        .last_seen_at
        .is_some_and(|last_seen| now.saturating_sub(last_seen) <= offline_after);
    AdminNodeResponse {
        id: row.node.public_id,
        name: row.node.name,
        region_code: row.node.region_code,
        sort_order: row.node.sort_order,
        last_ip: row.last_ip,
        online,
        last_seen_at: row.last_seen_at,
        cycle_rx: row.cycle_rx_bytes,
        cycle_tx: row.cycle_tx_bytes,
        traffic_limit: row.node.traffic_limit_bytes,
        price_micros: row.node.price_micros,
        currency: row.node.currency,
        renewal_cycle: row.node.renewal_cycle,
        expires_at: row.node.expires_at,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
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
        app::AppState,
        auth::{decode_hex, encode_hex},
        database::{Database, hydrate_startup},
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);
    const SESSION_RAW: [u8; 32] = [0x41; 32];
    const CSRF_RAW: [u8; 32] = [0x42; 32];

    struct TestContext {
        state: AppState,
        path: PathBuf,
        cookie: String,
        csrf: String,
    }

    impl TestContext {
        async fn new(default_reset_day: i64) -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("monitor-node-http-{}-{id}.db", std::process::id()));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open node test database");
            let mut settings = database.load_settings().await.expect("load settings");
            settings.default_traffic_reset_day = default_reset_day;
            settings.updated_at += 1;
            database
                .upsert_settings(settings)
                .await
                .expect("set default reset day");
            let admin_hash = "a".repeat(32);
            database
                .set_admin_password(admin_hash.clone(), 1)
                .await
                .expect("set administrator fixture");
            let now = unix_timestamp().expect("test timestamp");
            assert!(
                database
                    .create_session(admin_hash, sha256(&SESSION_RAW), now, now + 3_600)
                    .await
                    .expect("create session fixture")
            );
            let hydration = hydrate_startup(&database)
                .await
                .expect("hydrate test state");
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

        fn request(&self, body: &str, include_session: bool, include_csrf: bool) -> Request {
            let mut builder = HttpRequest::builder();
            if include_session {
                builder = builder.header("Cookie", &self.cookie);
            }
            if include_csrf {
                builder = builder
                    .header("Host", "monitor.test")
                    .header("Origin", "https://monitor.test")
                    .header("X-CSRF-Token", &self.csrf);
            }
            builder
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .expect("build test request")
        }

        fn read_request(&self, include_session: bool) -> Request {
            let mut builder = HttpRequest::builder();
            if include_session {
                builder = builder.header("Cookie", &self.cookie);
            }
            builder.body(Body::empty()).expect("build read request")
        }

        async fn finish(self) {
            self.state
                .database
                .shutdown()
                .await
                .expect("shutdown node test database");
            remove_database_files(&self.path);
        }
    }

    #[test]
    fn site_day_start_uses_iana_timezone_rules() {
        let shanghai_now: Timestamp = "2024-07-01T12:34:56Z".parse().expect("timestamp");
        let shanghai_expected: Timestamp = "2024-06-30T16:00:00Z".parse().expect("timestamp");
        assert_eq!(
            day_start_utc(shanghai_now.as_second(), "Asia/Shanghai")
                .unwrap_or_else(|_| panic!("Shanghai day start")),
            shanghai_expected.as_second()
        );

        let new_york_now: Timestamp = "2024-11-03T17:00:00Z".parse().expect("timestamp");
        let new_york_expected: Timestamp = "2024-11-03T04:00:00Z".parse().expect("timestamp");
        assert_eq!(
            day_start_utc(new_york_now.as_second(), "America/New_York")
                .unwrap_or_else(|_| panic!("New York day start")),
            new_york_expected.as_second()
        );
    }

    #[tokio::test]
    async fn create_persists_hash_hydrates_caches_and_uses_default_reset_day() {
        let context = TestContext::new(15).await;
        let body = json!({
            "name": "  Duplicate Name  ",
            "region_code": "US",
            "traffic_limit": 1_000,
            "price_micros": 39_900_000,
            "currency": "USD",
            "renewal_cycle": "annual",
            "expires_at": 1_800_000_000,
        })
        .to_string();
        let first = response(
            create(
                State(context.state.clone()),
                context.request(&body, true, true),
            )
            .await,
        );
        assert_eq!(first.status(), StatusCode::CREATED);
        let first_json = response_json(first).await;
        let public_id = text_field(&first_json["node"], "id");
        let plaintext_token = text_field(&first_json, "agent_token");
        assert!(valid_public_id(&public_id));
        assert_eq!(plaintext_token.len(), 64);
        assert_eq!(first_json["node"]["name"], "Duplicate Name");
        assert_eq!(first_json["node"]["traffic_reset_day"], 15);
        let raw_token = decode_hex(&plaintext_token).expect("agent token format");
        let expected_hash = sha256(&raw_token);

        let duplicate = response(
            create(
                State(context.state.clone()),
                context.request(&body, true, true),
            )
            .await,
        );
        assert_eq!(duplicate.status(), StatusCode::CREATED);

        let connection = Connection::open(&context.path).expect("inspect created node");
        let (node_id, name, sort_order): (i64, String, i64) = connection
            .query_row(
                "SELECT id, name, sort_order FROM nodes WHERE public_id = ?1",
                [&public_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("load created node");
        assert_eq!(name, "Duplicate Name");
        assert_eq!(sort_order, 0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM nodes WHERE name = ?1",
                    [&name],
                    |row| { row.get::<_, i64>(0) }
                )
                .expect("count duplicate names"),
            2
        );
        let (storage_type, stored_hash): (String, Vec<u8>) = connection
            .query_row(
                "SELECT typeof(token_hash), token_hash FROM node_tokens WHERE node_id = ?1",
                [node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load token hash");
        assert_eq!(storage_type, "blob");
        assert_eq!(stored_hash, expected_hash);
        assert_eq!(stored_hash.len(), 32);
        let totals: (i64, i64, Option<i64>, Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT rx_total_bytes, tx_total_bytes, last_rx_counter_bytes,
                        last_tx_counter_bytes, last_boot_id
                 FROM traffic_totals WHERE node_id = ?1",
                [node_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("load initial totals");
        assert_eq!(totals, (0, 0, None, None, None));
        drop(connection);

        assert_eq!(
            context
                .state
                .node_metadata
                .read()
                .await
                .get(&node_id)
                .map(|node| node.public_id.as_str()),
            Some(public_id.as_str())
        );
        assert_eq!(
            context.state.node_tokens.read().await.get(&expected_hash),
            Some(&node_id)
        );

        context
            .state
            .database
            .clone()
            .shutdown()
            .await
            .expect("shutdown before startup hydration test");
        let database = Database::open(&context.path).expect("reopen database");
        let hydration = hydrate_startup(&database).await.expect("rehydrate startup");
        let restarted = AppState::new(database, hydration);
        assert_eq!(
            restarted.node_tokens.read().await.get(&expected_hash),
            Some(&node_id)
        );
        restarted
            .database
            .shutdown()
            .await
            .expect("shutdown restarted database");
        remove_database_files(&context.path);
    }

    #[tokio::test]
    async fn create_validation_and_auth_contract_are_enforced() {
        let context = TestContext::new(1).await;
        let valid = json!({"name":"Node","region_code":"US"}).to_string();

        let unauthorized =
            response(list(State(context.state.clone()), context.read_request(false)).await);
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let missing_csrf = response(
            create(
                State(context.state.clone()),
                context.request(&valid, true, false),
            )
            .await,
        );
        assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
        let mut wrong_csrf = context.request(&valid, true, true);
        wrong_csrf.headers_mut().insert(
            "x-csrf-token",
            "0".repeat(64).parse().expect("valid test header"),
        );
        let wrong_csrf = response(create(State(context.state.clone()), wrong_csrf).await);
        assert_eq!(wrong_csrf.status(), StatusCode::FORBIDDEN);

        let invalid_bodies = [
            json!({"name":"   ","region_code":"US"}).to_string(),
            json!({"name":"x".repeat(65),"region_code":"US"}).to_string(),
            json!({"name":"Node","region_code":"us"}).to_string(),
            json!({"name":"Node","region_code":"USA"}).to_string(),
            json!({"name":"Node","region_code":"US","traffic_limit":0}).to_string(),
            json!({"name":"Node","region_code":"US","price_micros":1}).to_string(),
            json!({"name":"Node","region_code":"US","price_micros":1,"currency":"usd"}).to_string(),
            json!({"name":"Node","region_code":"US","renewal_cycle":"weekly"}).to_string(),
            json!({"name":"Node","region_code":"US","expires_at":9_007_199_254_740_992_i64})
                .to_string(),
            r#"{"name":"Node","region_code":"US","traffic_reset_day":null}"#.to_owned(),
            r#"{"name":"Node","region_code":"US","traffic_limit":1.5}"#.to_owned(),
            r#"{"name":"Node","region_code":"US","unknown":true}"#.to_owned(),
        ];
        for body in invalid_bodies {
            let response = response(
                create(
                    State(context.state.clone()),
                    context.request(&body, true, true),
                )
                .await,
            );
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        }

        let oversized = json!({
            "name": "x".repeat(8_300),
            "region_code": "US"
        })
        .to_string();
        let oversized_response = response(
            create(
                State(context.state.clone()),
                context.request(&oversized, true, true),
            )
            .await,
        );
        assert_eq!(oversized_response.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let mut wrong_type = context.request(&valid, true, true);
        wrong_type.headers_mut().remove(CONTENT_TYPE);
        let wrong_type_response = response(create(State(context.state.clone()), wrong_type).await);
        assert_eq!(
            wrong_type_response.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn admin_list_is_sorted_uses_joined_state_and_leaks_no_secrets() {
        let context = TestContext::new(1).await;
        let first = create_test_node(&context, "First", "US").await;
        let second = create_test_node(&context, "Second", "JP").await;
        let now = unix_timestamp().expect("test timestamp");
        let day_start =
            day_start_utc(now, "Asia/Shanghai").unwrap_or_else(|_| panic!("day start calculation"));
        let connection = Connection::open(&context.path).expect("open list fixtures");
        let first_internal: i64 = connection
            .query_row(
                "SELECT id FROM nodes WHERE public_id = ?1",
                [&first.0],
                |row| row.get(0),
            )
            .expect("first node id");
        insert_last_state(&connection, first_internal, now);
        connection
            .execute(
                "UPDATE traffic_totals SET rx_total_bytes = 30, tx_total_bytes = 40,
                    updated_at = ?2 WHERE node_id = ?1",
                params![first_internal, now],
            )
            .expect("update total traffic");
        connection
            .execute(
                "INSERT INTO traffic_daily
                    (node_id, day_start_utc, rx_bytes, tx_bytes, updated_at)
                 VALUES (?1, ?2, 10, 20, ?3)",
                params![first_internal, day_start, now],
            )
            .expect("insert daily traffic");
        connection
            .execute(
                "INSERT INTO traffic_cycles
                    (node_id, cycle_start_utc, cycle_end_utc, rx_bytes, tx_bytes, updated_at)
                 VALUES (?1, ?2, ?3, 50, 60, ?4)",
                params![first_internal, now - 60, now + 60, now],
            )
            .expect("insert cycle traffic");
        drop(connection);

        let joined = context
            .state
            .database
            .list_admin_nodes(day_start, now)
            .await
            .expect("run administrator list join");
        assert_eq!(joined[0].today_rx_bytes, 10);
        assert_eq!(joined[0].today_tx_bytes, 20);
        assert_eq!(joined[0].total_rx_bytes, 30);
        assert_eq!(joined[0].total_tx_bytes, 40);

        let response =
            response(list(State(context.state.clone()), context.read_request(true)).await);
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let nodes = body["nodes"].as_array().expect("node array");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["id"], first.0);
        assert_eq!(nodes[1]["id"], second.0);
        assert_eq!(nodes[0]["last_ip"], "203.0.113.10");
        assert_eq!(nodes[0]["online"], true);
        assert_eq!(nodes[0]["cycle_rx"], 50);
        assert_eq!(nodes[0]["cycle_tx"], 60);
        for node in nodes {
            for forbidden in [
                "token_hash",
                "agent_token",
                "boot_id",
                "last_rx_counter_bytes",
                "last_tx_counter_bytes",
                "session",
                "password_hash",
            ] {
                assert!(node.get(forbidden).is_none(), "leaked {forbidden}");
            }
        }
        context.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn patch_supports_tri_state_reorder_and_concurrent_cache_consistency() {
        let context = TestContext::new(1).await;
        let mut ids = Vec::new();
        for (name, region) in [("A", "US"), ("B", "JP"), ("C", "HK"), ("D", "SG")] {
            ids.push(create_test_node(&context, name, region).await.0);
        }

        let set_nullable = json!({
            "traffic_limit": 1_000,
            "price_micros": 10,
            "currency": "USD",
            "renewal_cycle": "annual",
            "expires_at": 1_800_000_000,
        })
        .to_string();
        assert_eq!(
            patch_node(&context, &ids[1], &set_nullable).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            patch_node(&context, &ids[1], r#"{"price_micros":null}"#)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let cleared = patch_node(
            &context,
            &ids[1],
            r#"{"traffic_limit":null,"price_micros":null,"currency":null,"renewal_cycle":null,"expires_at":null}"#,
        )
        .await;
        let cleared = response_json(cleared).await;
        assert_eq!(cleared["name"], "B");
        assert!(cleared["traffic_limit"].is_null());
        assert!(cleared["price_micros"].is_null());
        assert!(cleared["currency"].is_null());
        assert!(cleared["renewal_cycle"].is_null());
        assert!(cleared["expires_at"].is_null());
        assert_eq!(
            patch_node(&context, &ids[0], "{}").await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            patch_node(&context, &ids[0], r#"{"name":null}"#)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let malformed_id = response(
            patch(
                State(context.state.clone()),
                Path("not-a-public-id".to_owned()),
                context.request(r#"{"name":"Ignored"}"#, true, true),
            )
            .await,
        );
        assert_eq!(malformed_id.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            patch_node(&context, &ids[0], r#"{"sort_order":4}"#)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        assert_eq!(
            patch_node(&context, &ids[3], r#"{"sort_order":0}"#)
                .await
                .status(),
            StatusCode::OK
        );
        assert_orders(&context, &[&ids[3], &ids[0], &ids[1], &ids[2]]).await;
        assert_eq!(
            patch_node(&context, &ids[3], r#"{"sort_order":3}"#)
                .await
                .status(),
            StatusCode::OK
        );
        assert_orders(&context, &[&ids[0], &ids[1], &ids[2], &ids[3]]).await;

        let mut updates = Vec::new();
        for index in 0..12 {
            let state = context.state.clone();
            let public_id = ids[1].clone();
            let request = context.request(
                &json!({"name": format!("Concurrent {index}")}).to_string(),
                true,
                true,
            );
            updates.push(tokio::spawn(async move {
                response(patch(State(state), Path(public_id), request).await).status()
            }));
        }
        for update in updates {
            assert_eq!(update.await.expect("patch task completed"), StatusCode::OK);
        }
        let persisted = context
            .state
            .database
            .load_node_metadata()
            .await
            .expect("load persisted nodes")
            .into_iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        assert_eq!(*context.state.node_metadata.read().await, persisted);
        context.finish().await;
    }

    #[tokio::test]
    async fn rotate_token_replaces_database_and_cache_hash_atomically() {
        let context = TestContext::new(1).await;
        let (public_id, old_token) = create_test_node(&context, "Rotate", "DE").await;
        let old_raw = decode_hex(&old_token).expect("old token format");
        let old_hash = sha256(&old_raw);
        let response = response(
            rotate_token(
                State(context.state.clone()),
                Path(public_id.clone()),
                context.request("", true, true),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let new_token = text_field(&body, "agent_token");
        assert_ne!(new_token, old_token);
        let new_hash = sha256(&decode_hex(&new_token).expect("new token format"));
        let connection = Connection::open(&context.path).expect("inspect rotated token");
        let stored: Vec<u8> = connection
            .query_row(
                "SELECT t.token_hash FROM node_tokens AS t
                 JOIN nodes AS n ON n.id = t.node_id WHERE n.public_id = ?1",
                [&public_id],
                |row| row.get(0),
            )
            .expect("load rotated token hash");
        assert_eq!(stored, new_hash);
        drop(connection);
        let tokens = context.state.node_tokens.read().await;
        assert!(!tokens.contains_key(&old_hash));
        assert!(tokens.contains_key(&new_hash));
        drop(tokens);
        context.finish().await;
    }

    #[tokio::test]
    async fn delete_cascades_all_node_data_revokes_token_and_closes_order_gap() {
        let context = TestContext::new(1).await;
        let (public_id, token) = create_test_node(&context, "Delete", "US").await;
        let (remaining_id, _) = create_test_node(&context, "Remain", "JP").await;
        let token_hash = sha256(&decode_hex(&token).expect("token format"));
        let connection = Connection::open(&context.path).expect("open cascade fixtures");
        let node_id: i64 = connection
            .query_row(
                "SELECT id FROM nodes WHERE public_id = ?1",
                [&public_id],
                |row| row.get(0),
            )
            .expect("deleted node id");
        let now = unix_timestamp().expect("test timestamp");
        insert_last_state(&connection, node_id, now);
        connection
            .execute(
                "UPDATE traffic_totals SET rx_total_bytes = 1, tx_total_bytes = 2,
                    last_rx_counter_bytes = 3, last_tx_counter_bytes = 4,
                    last_boot_id = 'boot', updated_at = ?2 WHERE node_id = ?1",
                params![node_id, now],
            )
            .expect("update total fixture");
        connection
            .execute(
                "INSERT INTO traffic_daily VALUES (?1, 0, 1, 2, ?2)",
                params![node_id, now],
            )
            .expect("daily fixture");
        connection
            .execute(
                "INSERT INTO traffic_cycles VALUES (?1, 0, ?2, 1, 2, 1)",
                params![node_id, now + 60],
            )
            .expect("cycle fixture");
        connection
            .execute(
                "INSERT INTO node_history VALUES
                    (?1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0)",
                [node_id],
            )
            .expect("history fixture");
        connection
            .execute(
                "INSERT INTO ping_targets
                    (name, host, ip_family, enabled, sort_order, created_at, updated_at)
                 VALUES ('target', '192.0.2.1', 4, 1, 0, 1, 1)",
                [],
            )
            .expect("ping target fixture");
        let target_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO ping_history VALUES (?1, 0, ?2, 1, 0, NULL, NULL, NULL)",
                params![node_id, target_id],
            )
            .expect("ping history fixture");
        drop(connection);

        let response = response(
            delete(
                State(context.state.clone()),
                Path(public_id),
                context.request("", true, true),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let connection = Connection::open(&context.path).expect("inspect cascade deletion");
        for (table, id_column) in [
            ("nodes", "id"),
            ("node_tokens", "node_id"),
            ("node_last_state", "node_id"),
            ("traffic_totals", "node_id"),
            ("traffic_daily", "node_id"),
            ("traffic_cycles", "node_id"),
            ("node_history", "node_id"),
            ("ping_history", "node_id"),
        ] {
            let sql = format!("SELECT count(*) FROM {table} WHERE {id_column} = ?1");
            let count: i64 = connection
                .query_row(&sql, [node_id], |row| row.get(0))
                .expect("count cascade rows");
            assert_eq!(count, 0, "{table}");
        }
        let remaining_order: i64 = connection
            .query_row(
                "SELECT sort_order FROM nodes WHERE public_id = ?1",
                [&remaining_id],
                |row| row.get(0),
            )
            .expect("remaining node order");
        assert_eq!(remaining_order, 0);
        drop(connection);
        assert!(
            !context
                .state
                .node_tokens
                .read()
                .await
                .contains_key(&token_hash)
        );
        assert!(
            !context
                .state
                .node_metadata
                .read()
                .await
                .contains_key(&node_id)
        );
        context.finish().await;
    }

    fn response(result: Result<Response, ApiError>) -> Response {
        result.unwrap_or_else(IntoResponse::into_response)
    }

    async fn response_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 64 * 1_024)
            .await
            .expect("read response body");
        serde_json::from_slice(&bytes).expect("parse response JSON")
    }

    fn text_field(value: &Value, field: &str) -> String {
        value[field]
            .as_str()
            .expect("string response field")
            .to_owned()
    }

    async fn create_test_node(context: &TestContext, name: &str, region: &str) -> (String, String) {
        let body = json!({"name": name, "region_code": region}).to_string();
        let response = response(
            create(
                State(context.state.clone()),
                context.request(&body, true, true),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response_json(response).await;
        (
            text_field(&body["node"], "id"),
            text_field(&body, "agent_token"),
        )
    }

    async fn patch_node(context: &TestContext, public_id: &str, body: &str) -> Response {
        response(
            patch(
                State(context.state.clone()),
                Path(public_id.to_owned()),
                context.request(body, true, true),
            )
            .await,
        )
    }

    async fn assert_orders(context: &TestContext, public_ids: &[&String]) {
        let persisted = context
            .state
            .database
            .load_node_metadata()
            .await
            .expect("load node order");
        assert_eq!(
            persisted
                .iter()
                .map(|node| node.public_id.as_str())
                .collect::<Vec<_>>(),
            public_ids
                .iter()
                .map(|public_id| public_id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            persisted
                .iter()
                .map(|node| node.sort_order)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        let cache = context.state.node_metadata.read().await;
        for node in persisted {
            assert_eq!(cache.get(&node.id), Some(&node));
        }
    }

    fn insert_last_state(connection: &Connection, node_id: i64, now: i64) {
        connection
            .execute(
                "INSERT INTO node_last_state (
                    node_id, hostname, os_name, os_version, kernel, architecture,
                    cpu_model, cpu_cores, virtualization, agent_version, boot_id,
                    cpu_usage_bp, load_1_milli, load_5_milli, load_15_milli,
                    memory_total_bytes, memory_used_bytes, swap_total_bytes,
                    swap_used_bytes, disk_total_bytes, disk_used_bytes,
                    rx_rate_bytes_per_sec, tx_rate_bytes_per_sec, uptime_seconds,
                    process_count, last_ip, last_seen_at, persisted_at
                 ) VALUES (
                    ?1, 'host', 'Debian', '13', 'kernel', 'x86_64',
                    'CPU', 1, 'qemu', '0.1.0', 'boot',
                    100, 1, 1, 1, 1024, 512, 0, 0, 2048, 1024,
                    1, 2, 100, 10, '203.0.113.10', ?2, ?2
                 )",
                params![node_id, now],
            )
            .expect("insert last state fixture");
    }

    fn remove_database_files(path: &FsPath) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove node test database: {error}"),
            }
        }
    }
}
