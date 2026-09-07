use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderName, StatusCode, header::AUTHORIZATION},
    response::Response,
};
use monitor_common::{AgentConfigPayload, AgentReport};

use crate::{
    app::{AgentConfig, AppState},
    auth::{decode_hex, sha256, unix_timestamp},
    history::{PingSample, ResourceSample},
    snapshot::NodeSnapshot,
    time,
    traffic::TrafficSample,
};

use super::auth::{ApiError, json_response, no_content_response, parse_json};

const JSON_BODY_LIMIT: usize = 32 * 1_024;
const JS_SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;
const X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");

#[derive(Clone, Copy)]
struct AgentIdentity {
    node_id: i64,
    token_hash: [u8; 32],
}

pub(super) async fn config(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_agent(&state, request.headers()).await?;
    let config = state.agent_config.read().await;
    let response = AgentConfigPayload {
        protocol_version: 1,
        report_interval_seconds: config.report_interval_seconds,
        ping_interval_seconds: config.ping_interval_seconds,
        targets: config.targets.clone(),
    };
    drop(config);
    Ok(json_response(StatusCode::OK, response))
}

pub(super) async fn report(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Result<Response, ApiError> {
    reject_oversized_content_length(request.headers())?;
    let identity = authenticate_agent(&state, request.headers()).await?;
    let source_ip = source_ip(peer.ip(), request.headers())?;
    let report: AgentReport = parse_json(request, JSON_BODY_LIMIT).await?;
    {
        let config = state.agent_config.read().await;
        validate_report(&report, &config)?;
    }
    let received_at = unix_timestamp().map_err(|_| ApiError::internal())?;

    let wake_checkpoint = {
        let _lifecycle_guard = state.node_lifecycle_gate.read().await;
        let still_authenticated = state
            .node_tokens
            .read()
            .await
            .get(&identity.token_hash)
            .copied()
            == Some(identity.node_id);
        if !still_authenticated {
            return Err(ApiError::unauthorized());
        }
        let node = state
            .node_metadata
            .read()
            .await
            .get(&identity.node_id)
            .cloned()
            .ok_or_else(ApiError::unauthorized)?;
        let timezone = state.settings.read().await.site_timezone.clone();
        let day_start_utc = time::day_start_utc(received_at, &timezone).map_err(|error| {
            tracing::error!(timezone, error = %error, "site day start calculation failed");
            ApiError::internal()
        })?;
        let billing_cycle = time::billing_cycle(received_at, &timezone, node.traffic_reset_day)
            .map_err(|error| {
                tracing::error!(timezone, error = %error, "billing cycle calculation failed");
                ApiError::internal()
            })?;
        let mut snapshots = state.snapshots.write().await;
        let first_seen_at = snapshots
            .get(&identity.node_id)
            .map(|snapshot| snapshot.first_seen_at)
            .or(node.first_seen_at)
            .unwrap_or(received_at);
        let update = state
            .traffic
            .update(
                identity.node_id,
                TrafficSample {
                    rx_counter_bytes: report.network.rx_bytes,
                    tx_counter_bytes: report.network.tx_bytes,
                    boot_id: &report.boot_id,
                    day_start_utc,
                    billing_cycle,
                },
            )
            .map_err(|_| ApiError::internal())?;
        state.history.record(
            identity.node_id,
            received_at,
            ResourceSample {
                cpu_usage: report.cpu.usage,
                load_1: report.cpu.load_1,
                load_5: report.cpu.load_5,
                load_15: report.cpu.load_15,
                memory_used_bytes: report.memory.used,
                swap_used_bytes: report.memory.swap_used,
                disk_used_bytes: report.disk.used,
                rx_rate_bytes_per_sec: report.network.rx_rate,
                tx_rate_bytes_per_sec: report.network.tx_rate,
            },
            report.pings.iter().map(|ping| PingSample {
                target_id: ping.target_id,
                success: ping.success,
                latency_ms: ping.latency_ms,
            }),
        );
        let snapshot = report_into_snapshot(report, first_seen_at, received_at, source_ip);
        snapshots.insert(identity.node_id, snapshot);
        update.wake_checkpoint
    };
    if wake_checkpoint {
        state.traffic_flush.notify_one();
    }

    Ok(no_content_response())
}

async fn authenticate_agent(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AgentIdentity, ApiError> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next().ok_or_else(ApiError::unauthorized)?;
    if values.next().is_some() {
        return Err(ApiError::unauthorized());
    }
    let value = value.to_str().map_err(|_| ApiError::unauthorized())?;
    let token = value
        .strip_prefix("Bearer ")
        .and_then(decode_hex)
        .ok_or_else(ApiError::unauthorized)?;
    let token_hash = sha256(&token);
    let node_id = state
        .node_tokens
        .read()
        .await
        .get(&token_hash)
        .copied()
        .ok_or_else(ApiError::unauthorized)?;
    Ok(AgentIdentity {
        node_id,
        token_hash,
    })
}

fn reject_oversized_content_length(headers: &HeaderMap) -> Result<(), ApiError> {
    let mut values = headers.get_all(axum::http::header::CONTENT_LENGTH).iter();
    let Some(value) = values.next() else {
        return Ok(());
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_request());
    }
    let length = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(ApiError::invalid_request)?;
    if length > JSON_BODY_LIMIT as u64 {
        return Err(ApiError::payload_too_large(JSON_BODY_LIMIT));
    }
    Ok(())
}

/// Resolves the client address behind the frozen reverse-proxy contract: a
/// non-loopback direct peer always wins, and only a loopback peer may present a
/// single valid `X-Real-IP`. Shared with the login limiter so both paths agree.
pub(super) fn source_ip(peer: IpAddr, headers: &HeaderMap) -> Result<IpAddr, ApiError> {
    if !peer.is_loopback() {
        return Ok(peer);
    }
    let mut values = headers.get_all(&X_REAL_IP).iter();
    let Some(value) = values.next() else {
        return Ok(peer);
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_request());
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(ApiError::invalid_request)
}

fn validate_report(report: &AgentReport, config: &AgentConfig) -> Result<(), ApiError> {
    if report.protocol_version != 1
        || !valid_trimmed(&report.agent_version, 1, 32)
        || !valid_trimmed(&report.boot_id, 1, 64)
        || !valid_trimmed(&report.hostname, 1, 255)
        || !valid_trimmed(&report.os.name, 1, 128)
        || !valid_trimmed(&report.os.version, 0, 128)
        || !valid_trimmed(&report.os.kernel, 1, 255)
        || !matches!(report.os.architecture.trim(), "x86_64" | "aarch64")
        || !valid_trimmed(&report.os.virtualization, 1, 64)
        || !valid_trimmed(&report.cpu.model, 1, 255)
        || !(1..=4096).contains(&report.cpu.cores)
        || !valid_finite_range(report.cpu.usage, 0.0, 100.0)
        || !valid_nonnegative_float(report.cpu.load_1)
        || !valid_nonnegative_float(report.cpu.load_5)
        || !valid_nonnegative_float(report.cpu.load_15)
        || !valid_used_total(report.memory.used, report.memory.total)
        || !valid_used_total(report.memory.swap_used, report.memory.swap_total)
        || !valid_used_total(report.disk.used, report.disk.total)
        || !valid_counter(report.network.rx_bytes)
        || !valid_counter(report.network.tx_bytes)
        || !valid_safe_integer(report.network.rx_rate)
        || !valid_safe_integer(report.network.tx_rate)
        || !valid_safe_integer(report.uptime_seconds)
        || !valid_safe_integer(report.process_count)
        || report.pings.len() > 6
    {
        return Err(ApiError::invalid_request());
    }

    let mut target_ids = Vec::with_capacity(report.pings.len());
    for ping in &report.pings {
        if !valid_safe_integer(ping.target_id)
            || target_ids.contains(&ping.target_id)
            || !config
                .targets
                .iter()
                .any(|target| target.id == ping.target_id)
            || match (ping.success, ping.latency_ms) {
                (true, Some(latency)) => !valid_nonnegative_float(latency),
                (false, None) => false,
                _ => true,
            }
        {
            return Err(ApiError::invalid_request());
        }
        target_ids.push(ping.target_id);
    }
    Ok(())
}

fn valid_trimmed(value: &str, minimum: usize, maximum: usize) -> bool {
    let length = value.trim().chars().count();
    (minimum..=maximum).contains(&length)
}

fn valid_finite_range(value: f64, minimum: f64, maximum: f64) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

fn valid_nonnegative_float(value: f64) -> bool {
    value.is_finite() && (0.0..=(JS_SAFE_INTEGER_MAX as f64 / 1_000.0)).contains(&value)
}

fn valid_safe_integer(value: i64) -> bool {
    (0..=JS_SAFE_INTEGER_MAX).contains(&value)
}

/// Raw interface counters stay in `i64` all the way to SQLite and are never
/// serialized into the public snapshot, so only non-negativity is required.
/// Everything derived from them for the browser keeps the JS-safe bound.
fn valid_counter(value: i64) -> bool {
    value >= 0
}

fn valid_used_total(used: i64, total: i64) -> bool {
    valid_safe_integer(total) && valid_safe_integer(used) && used <= total
}

fn report_into_snapshot(
    report: AgentReport,
    first_seen_at: i64,
    last_seen_at: i64,
    last_ip: IpAddr,
) -> NodeSnapshot {
    NodeSnapshot {
        live_since_start: true,
        first_seen_at,
        last_seen_at,
        last_ip,
        hostname: report.hostname.trim().to_owned(),
        os_name: report.os.name.trim().to_owned(),
        os_version: report.os.version.trim().to_owned(),
        kernel: report.os.kernel.trim().to_owned(),
        architecture: report.os.architecture.trim().to_owned(),
        virtualization: report.os.virtualization.trim().to_owned(),
        agent_version: report.agent_version.trim().to_owned(),
        cpu_model: report.cpu.model.trim().to_owned(),
        cpu_cores: report.cpu.cores,
        cpu_usage: report.cpu.usage,
        load_1: report.cpu.load_1,
        load_5: report.cpu.load_5,
        load_15: report.cpu.load_15,
        memory_total: report.memory.total,
        memory_used: report.memory.used,
        swap_total: report.memory.swap_total,
        swap_used: report.memory.swap_used,
        disk_total: report.disk.total,
        disk_used: report.disk.used,
        rx_counter_bytes: report.network.rx_bytes,
        tx_counter_bytes: report.network.tx_bytes,
        rx_rate_bytes_per_sec: report.network.rx_rate,
        tx_rate_bytes_per_sec: report.network.tx_rate,
        uptime_seconds: report.uptime_seconds,
        process_count: report.process_count,
        boot_id: report.boot_id.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        body::{Body, to_bytes},
        extract::Path as AxumPath,
        http::{Request as HttpRequest, header::CONTENT_TYPE},
        response::IntoResponse,
    };
    use rusqlite::{Connection, params};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        app::AppState,
        auth::encode_hex,
        database::{Database, NewNodeRow, hydrate_startup},
        http::nodes,
        public_snapshot,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);
    const TOKEN_RAW: [u8; 32] = [0xab; 32];
    const SESSION_RAW: [u8; 32] = [0x41; 32];
    const CSRF_RAW: [u8; 32] = [0x42; 32];
    const PUBLIC_ID: &str = "11111111111111111111111111111111";

    struct TestContext {
        state: AppState,
        path: PathBuf,
        node_id: i64,
        token: String,
        admin_cookie: String,
        csrf: String,
        persisted_first_seen: Option<i64>,
        persisted_last_seen: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            Self::new_with_first_seen(true).await
        }

        async fn new_with_first_seen(persist_first_seen: bool) -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("monitor-agent-http-{}-{id}.db", std::process::id()));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open agent test database");
            let node = database
                .create_node(
                    NewNodeRow {
                        public_id: PUBLIC_ID.to_owned(),
                        name: "Agent node".to_owned(),
                        region_code: "US".to_owned(),
                        traffic_limit_bytes: None,
                        traffic_reset_day: 1,
                        price_micros: None,
                        currency: None,
                        renewal_cycle: None,
                        expires_at: None,
                    },
                    sha256(&TOKEN_RAW),
                    1,
                )
                .await
                .expect("create agent test node");
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
                    .expect("create administrator session")
            );
            database.shutdown().await.expect("close fixture database");

            let connection = Connection::open(&path).expect("open agent fixture connection");
            for (id, name, host, family, enabled, order) in [
                (1, "Enabled second", "203.0.113.1", 4, 1, 1),
                (2, "Disabled", "203.0.113.2", 4, 0, 0),
                (3, "Enabled first", "2001:db8::1", 6, 1, 0),
            ] {
                connection
                    .execute(
                        "INSERT INTO ping_targets
                            (id, name, host, ip_family, enabled, sort_order, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 1)",
                        params![id, name, host, family, enabled, order],
                    )
                    .expect("insert ping target fixture");
            }
            let persisted_first_seen = persist_first_seen.then_some(now - 3_600);
            if let Some(first_seen_at) = persisted_first_seen {
                connection
                    .execute(
                        "UPDATE nodes SET first_seen_at = ?2 WHERE id = ?1",
                        params![node.id, first_seen_at],
                    )
                    .expect("set persisted first-seen fixture");
            }
            let persisted_last_seen = now - 1;
            insert_last_state(&connection, node.id, persisted_last_seen);
            drop(connection);

            let database = Database::open(&path).expect("reopen agent test database");
            let hydration = hydrate_startup(&database)
                .await
                .expect("hydrate agent test state");
            let state = AppState::new(database, hydration);
            let csrf = encode_hex(&CSRF_RAW);
            let admin_cookie = format!(
                "__Host-monitor_session={}; __Host-monitor_csrf={csrf}",
                encode_hex(&SESSION_RAW)
            );
            Self {
                state,
                path,
                node_id: node.id,
                token: encode_hex(&TOKEN_RAW),
                admin_cookie,
                csrf,
                persisted_first_seen,
                persisted_last_seen,
            }
        }

        fn report_request(&self, body: &str) -> Request {
            agent_request(&self.token, body)
        }

        fn admin_request(&self) -> Request {
            HttpRequest::builder()
                .header("Cookie", &self.admin_cookie)
                .header("Host", "monitor.test")
                .header("Origin", "https://monitor.test")
                .header("X-CSRF-Token", &self.csrf)
                .body(Body::empty())
                .expect("build administrator request")
        }

        fn admin_read_request(&self) -> Request {
            HttpRequest::builder()
                .header("Cookie", &self.admin_cookie)
                .body(Body::empty())
                .expect("build administrator read request")
        }

        async fn finish(self) {
            self.state
                .database
                .shutdown()
                .await
                .expect("shutdown agent test database");
            remove_database_files(&self.path);
        }
    }

    #[tokio::test]
    async fn bearer_auth_is_strict_and_uses_cached_raw_token_hash() {
        let context = TestContext::new().await;
        let valid = response(
            config(
                State(context.state.clone()),
                agent_read_request(Some(&format!("Bearer {}", context.token))),
            )
            .await,
        );
        assert_eq!(valid.status(), StatusCode::OK);
        assert_eq!(
            valid.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );

        for authorization in [
            None,
            Some("Bearer ab"),
            Some(&format!("Bearer {}", context.token.to_uppercase())),
            Some(&format!("Bearer {}", "g".repeat(64))),
            Some(&format!("Bearer {}", encode_hex(&[0xcd; 32]))),
        ] {
            let denied = response(
                config(
                    State(context.state.clone()),
                    agent_read_request(authorization),
                )
                .await,
            );
            assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response_json(denied).await["error"]["code"], "unauthorized");
        }
        let mut duplicate = agent_read_request(Some(&format!("Bearer {}", context.token)));
        duplicate.headers_mut().append(
            AUTHORIZATION,
            format!("Bearer {}", context.token)
                .parse()
                .expect("authorization header"),
        );
        assert_eq!(
            response(config(State(context.state.clone()), duplicate).await).status(),
            StatusCode::UNAUTHORIZED
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn config_and_report_remain_available_after_database_worker_stops() {
        let context = TestContext::new().await;
        context
            .state
            .database
            .clone()
            .shutdown()
            .await
            .expect("stop database worker");

        let config_response = response(
            config(
                State(context.state.clone()),
                agent_read_request(Some(&format!("Bearer {}", context.token))),
            )
            .await,
        );
        assert_eq!(config_response.status(), StatusCode::OK);
        let report_response = response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                context.report_request(&valid_report().to_string()),
            )
            .await,
        );
        assert_eq!(report_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            report_response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
        assert!(
            context
                .state
                .snapshots
                .read()
                .await
                .contains_key(&context.node_id)
        );
        let mut next = valid_report();
        next["network"]["rx_bytes"] = json!(150);
        next["network"]["tx_bytes"] = json!(260);
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&next.to_string()),
                )
                .await
            )
            .status(),
            StatusCode::NO_CONTENT
        );
        let traffic = context
            .state
            .traffic
            .get(context.node_id)
            .expect("traffic without database worker");
        assert_eq!((traffic.rx_total_bytes, traffic.tx_total_bytes), (50, 60));
        assert_eq!(context.state.history.resource_samples(context.node_id), 2);

        let path = context.path.clone();
        drop(context);
        remove_database_files(&path);
    }

    #[tokio::test]
    async fn report_validation_body_contract_and_receipt_time_are_enforced() {
        let context = TestContext::new().await;
        let mut invalid_reports = vec![
            with_value("protocol_version", json!(2)),
            with_nested_value("os", "extra", json!(true)),
            with_nested_value("os", "architecture", json!("amd64")),
            with_nested_value("cpu", "usage", json!(100.1)),
            with_nested_value("cpu", "load_1", json!(-0.1)),
            with_nested_value("memory", "used", json!(2_000)),
            with_nested_value("network", "rx_bytes", json!(-1)),
            with_nested_value("network", "tx_bytes", json!(-1)),
            with_nested_value("network", "rx_rate", json!(9_007_199_254_740_992_i64)),
            with_nested_value("network", "tx_rate", json!(9_007_199_254_740_992_i64)),
            with_value("uptime_seconds", json!(9_007_199_254_740_992_i64)),
        ];
        let mut duplicate_ping = valid_report();
        duplicate_ping["pings"] = json!([
            {"target_id":3,"success":true,"latency_ms":12.5},
            {"target_id":3,"success":false,"latency_ms":null}
        ]);
        invalid_reports.push(duplicate_ping);
        let mut disabled_ping = valid_report();
        disabled_ping["pings"][0]["target_id"] = json!(2);
        invalid_reports.push(disabled_ping);
        let mut unknown_ping = valid_report();
        unknown_ping["pings"][0]["target_id"] = json!(999);
        invalid_reports.push(unknown_ping);
        let mut successful_without_latency = valid_report();
        successful_without_latency["pings"][0]["latency_ms"] = Value::Null;
        invalid_reports.push(successful_without_latency);
        let mut failed_with_latency = valid_report();
        failed_with_latency["pings"][0]["success"] = json!(false);
        invalid_reports.push(failed_with_latency);
        invalid_reports.push(with_value("timestamp", json!(1)));

        for invalid in invalid_reports {
            let result = response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&invalid.to_string()),
                )
                .await,
            );
            assert_eq!(result.status(), StatusCode::BAD_REQUEST, "{invalid}");
        }

        let oversized = json!({"padding":"x".repeat(JSON_BODY_LIMIT + 1)}).to_string();
        let oversized = response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                context.report_request(&oversized),
            )
            .await,
        );
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let mut declared_oversized = context.report_request(&valid_report().to_string());
        declared_oversized.headers_mut().insert(
            axum::http::header::CONTENT_LENGTH,
            (JSON_BODY_LIMIT + 1)
                .to_string()
                .parse()
                .expect("content length header"),
        );
        let declared_oversized = response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                declared_oversized,
            )
            .await,
        );
        assert_eq!(declared_oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let mut wrong_content_type = context.report_request(&valid_report().to_string());
        wrong_content_type.headers_mut().remove(CONTENT_TYPE);
        let wrong_content_type = response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                wrong_content_type,
            )
            .await,
        );
        assert_eq!(
            wrong_content_type.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );

        assert!(!valid_nonnegative_float(f64::NAN));
        assert!(!valid_nonnegative_float(f64::INFINITY));
        let before = unix_timestamp().expect("time before report");
        let accepted = response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                context.report_request(&valid_report().to_string()),
            )
            .await,
        );
        let after = unix_timestamp().expect("time after report");
        assert_eq!(accepted.status(), StatusCode::NO_CONTENT);
        let snapshot = context.state.snapshots.read().await[&context.node_id].clone();
        assert!((before..=after).contains(&snapshot.last_seen_at));
        assert_eq!(
            snapshot.first_seen_at,
            context.persisted_first_seen.unwrap()
        );
        assert!(snapshot.live_since_start);
        context.finish().await;
    }

    #[tokio::test]
    async fn raw_network_counters_beyond_the_js_safe_range_are_accepted() {
        // Raw interface counters only travel Agent -> server -> SQLite, all i64,
        // and never reach the browser, so a long-lived host must not be locked
        // out at 2^53 while every browser-visible value stays JS-safe.
        let context = TestContext::new().await;
        const BEYOND_JS_SAFE: i64 = 9_007_199_254_740_991 + 100;

        let mut huge = valid_report();
        huge["network"]["rx_bytes"] = json!(BEYOND_JS_SAFE);
        huge["network"]["tx_bytes"] = json!(i64::MAX);
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&huge.to_string()),
                )
                .await,
            )
            .status(),
            StatusCode::NO_CONTENT
        );
        let snapshot = context.state.snapshots.read().await[&context.node_id].clone();
        assert_eq!(snapshot.rx_counter_bytes, BEYOND_JS_SAFE);
        assert_eq!(snapshot.tx_counter_bytes, i64::MAX);

        // A same-boot monotonic step from that baseline yields the real delta,
        // and the browser-visible total stays inside the JS-safe range.
        let mut stepped = valid_report();
        stepped["network"]["rx_bytes"] = json!(BEYOND_JS_SAFE + 1_000);
        stepped["network"]["tx_bytes"] = json!(i64::MAX);
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&stepped.to_string()),
                )
                .await,
            )
            .status(),
            StatusCode::NO_CONTENT
        );
        let traffic = context.state.traffic.read_current();
        let node = traffic[&context.node_id];
        assert_eq!(node.rx_total_bytes, 1_000);
        assert_eq!(node.tx_total_bytes, 0);
        assert!(node.rx_total_bytes <= crate::traffic::JS_SAFE_INTEGER_MAX);
        context.finish().await;
    }

    #[tokio::test]
    async fn report_crossing_browser_safe_total_remains_successful() {
        let context = TestContext::new().await;
        let mut baseline = valid_report();
        baseline["network"]["rx_bytes"] = json!(0);
        baseline["network"]["tx_bytes"] = json!(0);
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&baseline.to_string()),
                )
                .await
            )
            .status(),
            StatusCode::NO_CONTENT
        );

        let mut near_limit = valid_report();
        near_limit["network"]["rx_bytes"] = json!(JS_SAFE_INTEGER_MAX - 100);
        near_limit["network"]["tx_bytes"] = json!(JS_SAFE_INTEGER_MAX - 100);
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&near_limit.to_string()),
                )
                .await
            )
            .status(),
            StatusCode::NO_CONTENT
        );

        let mut crossing = valid_report();
        crossing["network"]["rx_bytes"] = json!(JS_SAFE_INTEGER_MAX + 900);
        crossing["network"]["tx_bytes"] = json!(JS_SAFE_INTEGER_MAX + 900);
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&crossing.to_string()),
                )
                .await
            )
            .status(),
            StatusCode::NO_CONTENT
        );
        let traffic = context
            .state
            .traffic
            .get(context.node_id)
            .expect("crossing traffic state");
        assert!(traffic.rx_total_bytes > JS_SAFE_INTEGER_MAX);
        assert_eq!(traffic.rx_total_bytes, JS_SAFE_INTEGER_MAX + 900);
        let snapshot = context.state.snapshots.read().await[&context.node_id].clone();
        assert!(snapshot.last_seen_at > 0);
        context.finish().await;
    }

    #[tokio::test]
    async fn true_i64_traffic_overflow_is_an_internal_error() {
        let context = TestContext::new().await;
        for rx in [0, i64::MAX] {
            let mut report_body = valid_report();
            report_body["network"]["rx_bytes"] = json!(rx);
            report_body["network"]["tx_bytes"] = json!(0);
            assert_eq!(
                response(
                    report(
                        State(context.state.clone()),
                        ConnectInfo(loopback_peer()),
                        context.report_request(&report_body.to_string()),
                    )
                    .await
                )
                .status(),
                StatusCode::NO_CONTENT
            );
        }
        let mut overflowing = valid_report();
        overflowing["network"]["rx_bytes"] = json!(1);
        overflowing["network"]["tx_bytes"] = json!(0);
        let response = response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                context.report_request(&overflowing.to_string()),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "internal_error"
        );
        assert_eq!(
            context
                .state
                .traffic
                .get(context.node_id)
                .expect("overflow state")
                .rx_total_bytes,
            i64::MAX
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn reports_overwrite_one_snapshot_and_preserve_first_seen() {
        let context = TestContext::new().await;
        let first_seen_at = context.persisted_first_seen.unwrap();
        for index in 0..20 {
            let mut body = valid_report();
            body["hostname"] = json!(format!("host-{index}"));
            body["cpu"]["usage"] = json!(index as f64);
            let result = response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    context.report_request(&body.to_string()),
                )
                .await,
            );
            assert_eq!(result.status(), StatusCode::NO_CONTENT);
            let current_first_seen =
                context.state.snapshots.read().await[&context.node_id].first_seen_at;
            assert_eq!(current_first_seen, first_seen_at);
        }
        let snapshots = context.state.snapshots.read().await;
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[&context.node_id];
        assert_eq!(snapshot.hostname, "host-19");
        assert_eq!(snapshot.cpu_usage, 19.0);
        assert_eq!(snapshot.first_seen_at, first_seen_at);
        assert!(snapshot.live_since_start);
        drop(snapshots);
        context.finish().await;
    }

    #[tokio::test]
    async fn source_ip_only_trusts_single_x_real_ip_from_loopback() {
        let context = TestContext::new().await;
        let mut proxied = context.report_request(&valid_report().to_string());
        proxied
            .headers_mut()
            .insert(&X_REAL_IP, "198.51.100.20".parse().expect("valid header"));
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    proxied,
                )
                .await
            )
            .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            context.state.snapshots.read().await[&context.node_id].last_ip,
            "198.51.100.20".parse::<IpAddr>().expect("valid IP")
        );

        let mut malformed = context.report_request(&valid_report().to_string());
        malformed
            .headers_mut()
            .insert(&X_REAL_IP, "not-an-ip".parse().expect("valid header bytes"));
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    malformed,
                )
                .await
            )
            .status(),
            StatusCode::BAD_REQUEST
        );
        let mut multiple = context.report_request(&valid_report().to_string());
        multiple
            .headers_mut()
            .append(&X_REAL_IP, "198.51.100.1".parse().expect("header"));
        multiple
            .headers_mut()
            .append(&X_REAL_IP, "198.51.100.2".parse().expect("header"));
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    multiple,
                )
                .await
            )
            .status(),
            StatusCode::BAD_REQUEST
        );

        let mut forged = context.report_request(&valid_report().to_string());
        forged
            .headers_mut()
            .append(&X_REAL_IP, "invalid".parse().expect("header"));
        forged
            .headers_mut()
            .append(&X_REAL_IP, "also-invalid".parse().expect("header"));
        let peer = SocketAddr::from(([192, 0, 2, 40], 30_000));
        assert_eq!(
            response(report(State(context.state.clone()), ConnectInfo(peer), forged).await)
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            context.state.snapshots.read().await[&context.node_id].last_ip,
            peer.ip()
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn startup_config_contains_only_sorted_enabled_targets_and_rejects_more_than_six() {
        let context = TestContext::new().await;
        let response = response(
            config(
                State(context.state.clone()),
                agent_read_request(Some(&format!("Bearer {}", context.token))),
            )
            .await,
        );
        let body = response_json(response).await;
        assert_eq!(body["protocol_version"], 1);
        assert_eq!(body["report_interval_seconds"], 2);
        assert_eq!(body["ping_interval_seconds"], 15);
        assert_eq!(body["targets"].as_array().expect("targets").len(), 2);
        assert_eq!(body["targets"][0]["id"], 3);
        assert_eq!(body["targets"][1]["id"], 1);
        context.finish().await;

        let path = test_path("too-many-targets");
        remove_database_files(&path);
        Database::open(&path)
            .expect("create target limit database")
            .shutdown()
            .await
            .expect("close target limit database");
        let connection = Connection::open(&path).expect("open target limit fixture");
        for id in 1..=7 {
            connection
                .execute(
                    "INSERT INTO ping_targets
                        (id, name, host, ip_family, enabled, sort_order, created_at, updated_at)
                     VALUES (?1, ?2, '203.0.113.1', 4, 1, ?1, 1, 1)",
                    params![id, format!("Target {id}")],
                )
                .expect("insert excess enabled target");
        }
        drop(connection);
        let database = Database::open(&path).expect("reopen target limit database");
        assert!(matches!(
            hydrate_startup(&database).await,
            Err(crate::database::DatabaseError::TooManyEnabledPingTargets { count: 7 })
        ));
        database.shutdown().await.expect("shutdown target database");
        remove_database_files(&path);
    }

    #[tokio::test]
    async fn delete_revokes_agent_and_removes_snapshot() {
        let context = TestContext::new().await;
        assert_eq!(
            send_valid_report(&context, &context.token).await,
            StatusCode::NO_CONTENT
        );
        assert!(
            context
                .state
                .snapshots
                .read()
                .await
                .contains_key(&context.node_id)
        );
        let deleted = response(
            nodes::delete(
                State(context.state.clone()),
                AxumPath(PUBLIC_ID.to_owned()),
                context.admin_request(),
            )
            .await,
        );
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        assert!(
            !context
                .state
                .node_tokens
                .read()
                .await
                .contains_key(&sha256(&TOKEN_RAW))
        );
        assert!(
            !context
                .state
                .node_metadata
                .read()
                .await
                .contains_key(&context.node_id)
        );
        assert!(
            !context
                .state
                .snapshots
                .read()
                .await
                .contains_key(&context.node_id)
        );
        assert!(context.state.traffic.get(context.node_id).is_none());
        assert_eq!(context.state.history.resource_samples(context.node_id), 0);
        assert_eq!(
            send_valid_report(&context, &context.token).await,
            StatusCode::UNAUTHORIZED
        );
        assert!(context.state.snapshots.read().await.is_empty());
        context.finish().await;
    }

    #[tokio::test]
    async fn rotation_revokes_old_token_and_accepts_new_without_dropping_snapshot() {
        let context = TestContext::new().await;
        assert_eq!(
            send_valid_report(&context, &context.token).await,
            StatusCode::NO_CONTENT
        );
        let rotated = response(
            nodes::rotate_token(
                State(context.state.clone()),
                AxumPath(PUBLIC_ID.to_owned()),
                context.admin_request(),
            )
            .await,
        );
        assert_eq!(rotated.status(), StatusCode::OK);
        let new_token = response_json(rotated).await["agent_token"]
            .as_str()
            .expect("new agent token")
            .to_owned();
        assert_eq!(
            send_valid_report(&context, &context.token).await,
            StatusCode::UNAUTHORIZED
        );
        assert!(
            context
                .state
                .snapshots
                .read()
                .await
                .contains_key(&context.node_id)
        );
        assert_eq!(
            send_valid_report(&context, &new_token).await,
            StatusCode::NO_CONTENT
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn admin_list_prefers_realtime_snapshot_and_falls_back_to_persisted_state() {
        let context = TestContext::new().await;
        let persisted = context.state.snapshots.read().await[&context.node_id].clone();
        assert!(!persisted.live_since_start);
        assert_eq!(
            persisted.first_seen_at,
            context.persisted_first_seen.unwrap()
        );
        assert_eq!(persisted.last_seen_at, context.persisted_last_seen);

        public_snapshot::generate(&context.state, context.persisted_last_seen + 1)
            .await
            .expect("restart public snapshot");
        let cached = context.state.public_snapshot.load().await;
        let public: Value = serde_json::from_slice(&cached.body).expect("public snapshot JSON");
        assert_eq!(public["nodes"][0]["online"], false);
        assert_eq!(
            public["nodes"][0]["last_seen_at"],
            context.persisted_last_seen
        );
        assert_eq!(
            public["nodes"][0]["first_seen_at"],
            context.persisted_first_seen.unwrap()
        );
        assert_eq!(public["nodes"][0]["system"]["hostname"], "host");
        assert_eq!(public["nodes"][0]["metrics"]["cpu_usage"], 1.0);

        let fallback =
            response(nodes::list(State(context.state.clone()), context.admin_read_request()).await);
        let fallback = response_json(fallback).await;
        assert_eq!(fallback["nodes"][0]["last_ip"], "203.0.113.10");
        assert_eq!(
            fallback["nodes"][0]["last_seen_at"],
            context.persisted_last_seen
        );
        assert_eq!(fallback["nodes"][0]["online"], false);

        let mut realtime = context.report_request(&valid_report().to_string());
        realtime.headers_mut().insert(
            &X_REAL_IP,
            "198.51.100.70".parse().expect("realtime IP header"),
        );
        assert_eq!(
            response(
                report(
                    State(context.state.clone()),
                    ConnectInfo(loopback_peer()),
                    realtime,
                )
                .await
            )
            .status(),
            StatusCode::NO_CONTENT
        );
        let realtime =
            response(nodes::list(State(context.state.clone()), context.admin_read_request()).await);
        let realtime = response_json(realtime).await;
        assert_eq!(realtime["nodes"][0]["last_ip"], "198.51.100.70");
        assert_eq!(
            realtime["nodes"][0]["last_seen_at"],
            context.state.snapshots.read().await[&context.node_id].last_seen_at
        );
        assert_eq!(realtime["nodes"][0]["online"], true);
        let live = context.state.snapshots.read().await[&context.node_id].clone();
        assert!(live.live_since_start);
        public_snapshot::generate(&context.state, live.last_seen_at)
            .await
            .expect("live public snapshot");
        let cached = context.state.public_snapshot.load().await;
        let public: Value = serde_json::from_slice(&cached.body).expect("live public JSON");
        assert_eq!(public["nodes"][0]["online"], true);
        assert_eq!(public["nodes"][0]["system"]["hostname"], "dmit-01");
        context.finish().await;
    }

    #[tokio::test]
    async fn inconsistent_last_state_without_first_seen_uses_last_seen_fallback() {
        let context = TestContext::new_with_first_seen(false).await;
        let snapshot = context.state.snapshots.read().await[&context.node_id].clone();
        assert!(!snapshot.live_since_start);
        assert_eq!(snapshot.first_seen_at, context.persisted_last_seen);
        context.finish().await;
    }

    fn valid_report() -> Value {
        json!({
            "protocol_version": 1,
            "agent_version": "0.1.0",
            "boot_id": "37e3a670-fdde-4173-8728-a7d6b727b590",
            "hostname": "dmit-01",
            "os": {
                "name": "Debian",
                "version": "13",
                "kernel": "6.12.107+deb13-amd64",
                "architecture": "x86_64",
                "virtualization": "qemu"
            },
            "cpu": {
                "model": "AMD EPYC 9654",
                "cores": 1,
                "usage": 0.8,
                "load_1": 0.01,
                "load_5": 0.0,
                "load_15": 0.0
            },
            "memory": {"total": 1024, "used": 512, "swap_total": 0, "swap_used": 0},
            "disk": {"total": 2048, "used": 1024},
            "network": {"rx_bytes": 100, "tx_bytes": 200, "rx_rate": 10, "tx_rate": 20},
            "uptime_seconds": 3600,
            "process_count": 109,
            "pings": [{"target_id":3,"success":true,"latency_ms":142.1}]
        })
    }

    fn with_value(field: &str, value: Value) -> Value {
        let mut report = valid_report();
        report[field] = value;
        report
    }

    fn with_nested_value(object: &str, field: &str, value: Value) -> Value {
        let mut report = valid_report();
        report[object][field] = value;
        report
    }

    fn agent_request(token: &str, body: &str) -> Request {
        HttpRequest::builder()
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .expect("build agent report request")
    }

    fn agent_read_request(authorization: Option<&str>) -> Request {
        let mut builder = HttpRequest::builder();
        if let Some(authorization) = authorization {
            builder = builder.header(AUTHORIZATION, authorization);
        }
        builder
            .body(Body::empty())
            .expect("build agent read request")
    }

    async fn send_valid_report(context: &TestContext, token: &str) -> StatusCode {
        response(
            report(
                State(context.state.clone()),
                ConnectInfo(loopback_peer()),
                agent_request(token, &valid_report().to_string()),
            )
            .await,
        )
        .status()
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

    fn loopback_peer() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 30_000))
    }

    fn test_path(label: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "monitor-agent-{label}-{}-{id}.db",
            std::process::id()
        ))
    }

    fn insert_last_state(connection: &Connection, node_id: i64, last_seen_at: i64) {
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
                    'CPU', 1, 'qemu', '0.1.0', 'boot', 100, 1, 1, 1,
                    1024, 512, 0, 0, 2048, 1024, 1, 2, 100, 10,
                    '203.0.113.10', ?2, ?2
                 )",
                params![node_id, last_seen_at],
            )
            .expect("insert persisted last state");
    }

    fn remove_database_files(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove agent test database: {error}"),
            }
        }
    }
}
