use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
    },
    response::Response,
};
use serde::Serialize;

use crate::{
    app::AppState,
    auth::{encode_hex, sha256, unix_timestamp},
    database::{PingHistoryPoint, ResourceHistoryPoint},
};

use super::auth::ApiError;

const HISTORY_CACHE_CONTROL: &str = "public, max-age=30";

#[derive(Clone, Copy)]
struct HistoryRange {
    duration: i64,
    step: i64,
}

#[derive(Serialize)]
struct ResourceHistoryResponse {
    node_id: String,
    from: i64,
    to: i64,
    step: i64,
    series: ResourceSeries,
}

#[derive(Serialize)]
struct ResourceSeries {
    timestamp: Vec<i64>,
    cpu: Vec<Option<f64>>,
    memory: Vec<Option<i64>>,
    disk: Vec<Option<i64>>,
    rx_rate: Vec<Option<i64>>,
    tx_rate: Vec<Option<i64>>,
}

#[derive(Serialize)]
struct PingHistoryResponse {
    node_id: String,
    from: i64,
    to: i64,
    step: i64,
    targets: Vec<PingTargetResponse>,
    timestamps: Vec<i64>,
    series: Vec<PingSeries>,
}

#[derive(Serialize)]
struct PingTargetResponse {
    id: i64,
    name: String,
    ip_family: i64,
    sort_order: i64,
}

#[derive(Serialize)]
struct PingSeries {
    target_id: i64,
    latency: Vec<Option<f64>>,
}

pub(super) async fn resource(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let range = parse_range(request.uri().query())?;
    let node_id = resolve_node(&state, &public_id).await?;
    let to = unix_timestamp().map_err(|_| ApiError::internal())?;
    let from = to - range.duration;
    let points = state
        .database
        .query_resource_history(node_id, from, to, range.step)
        .await
        .map_err(ApiError::database)?;
    let response = ResourceHistoryResponse {
        node_id: public_id,
        from,
        to,
        step: range.step,
        series: dense_resource_series(from, to, range.step, points),
    };
    history_response(request.headers(), &response)
}

pub(super) async fn ping(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let range = parse_range(request.uri().query())?;
    let node_id = resolve_node(&state, &public_id).await?;
    let to = unix_timestamp().map_err(|_| ApiError::internal())?;
    let from = to - range.duration;
    let points = state
        .database
        .query_ping_history(node_id, from, to, range.step)
        .await
        .map_err(ApiError::database)?;
    let (targets, timestamps, series) = dense_ping_series(from, to, range.step, points);
    let response = PingHistoryResponse {
        node_id: public_id,
        from,
        to,
        step: range.step,
        targets,
        timestamps,
        series,
    };
    history_response(request.headers(), &response)
}

async fn resolve_node(state: &AppState, public_id: &str) -> Result<i64, ApiError> {
    if public_id.len() != 32
        || !public_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ApiError::not_found());
    }
    state
        .node_metadata
        .read()
        .await
        .values()
        .find(|node| node.public_id == public_id)
        .map(|node| node.id)
        .ok_or_else(ApiError::not_found)
}

fn parse_range(query: Option<&str>) -> Result<HistoryRange, ApiError> {
    let query = query.ok_or_else(ApiError::invalid_request)?;
    let mut parts = query.split('&');
    let first = parts.next().ok_or_else(ApiError::invalid_request)?;
    if parts.next().is_some() {
        return Err(ApiError::invalid_request());
    }
    let value = first
        .strip_prefix("range=")
        .filter(|_| first.matches('=').count() == 1)
        .ok_or_else(ApiError::invalid_request)?;
    match value {
        "1h" => Ok(HistoryRange {
            duration: 60 * 60,
            step: 60,
        }),
        "6h" => Ok(HistoryRange {
            duration: 6 * 60 * 60,
            step: 60,
        }),
        "24h" => Ok(HistoryRange {
            duration: 24 * 60 * 60,
            step: 60,
        }),
        "7d" => Ok(HistoryRange {
            duration: 7 * 24 * 60 * 60,
            step: 300,
        }),
        _ => Err(ApiError::invalid_request()),
    }
}

fn aligned_timestamps(from: i64, to: i64, step: i64) -> Vec<i64> {
    let first = from + (step - from.rem_euclid(step)).rem_euclid(step);
    (first..to).step_by(step as usize).collect()
}

fn dense_resource_series(
    from: i64,
    to: i64,
    step: i64,
    points: Vec<ResourceHistoryPoint>,
) -> ResourceSeries {
    let points: HashMap<_, _> = points
        .into_iter()
        .map(|point| (point.bucket_ts, point))
        .collect();
    let timestamp = aligned_timestamps(from, to, step);
    let mut cpu = Vec::with_capacity(timestamp.len());
    let mut memory = Vec::with_capacity(timestamp.len());
    let mut disk = Vec::with_capacity(timestamp.len());
    let mut rx_rate = Vec::with_capacity(timestamp.len());
    let mut tx_rate = Vec::with_capacity(timestamp.len());
    for bucket in &timestamp {
        let point = points.get(bucket);
        cpu.push(point.map(|point| point.cpu_usage));
        memory.push(point.map(|point| point.memory_used_bytes));
        disk.push(point.map(|point| point.disk_used_bytes));
        rx_rate.push(point.map(|point| point.rx_rate_bytes_per_sec));
        tx_rate.push(point.map(|point| point.tx_rate_bytes_per_sec));
    }
    ResourceSeries {
        timestamp,
        cpu,
        memory,
        disk,
        rx_rate,
        tx_rate,
    }
}

fn dense_ping_series(
    from: i64,
    to: i64,
    step: i64,
    points: Vec<PingHistoryPoint>,
) -> (Vec<PingTargetResponse>, Vec<i64>, Vec<PingSeries>) {
    let timestamps = aligned_timestamps(from, to, step);
    let positions: HashMap<_, _> = timestamps
        .iter()
        .enumerate()
        .map(|(index, timestamp)| (*timestamp, index))
        .collect();
    let mut targets = Vec::new();
    let mut series: Vec<PingSeries> = Vec::new();
    let mut target_indexes = HashMap::new();
    for point in points {
        let index = *target_indexes.entry(point.target_id).or_insert_with(|| {
            let index = targets.len();
            targets.push(PingTargetResponse {
                id: point.target_id,
                name: point.name.clone(),
                ip_family: point.ip_family,
                sort_order: point.sort_order,
            });
            series.push(PingSeries {
                target_id: point.target_id,
                latency: vec![None; timestamps.len()],
            });
            index
        });
        if let Some(bucket_ts) = point.bucket_ts
            && let Some(position) = positions.get(&bucket_ts)
        {
            series[index].latency[*position] = point.latency_ms;
        }
    }
    (targets, timestamps, series)
}

fn history_response(
    request_headers: &HeaderMap,
    value: &impl Serialize,
) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|_| ApiError::internal())?;
    let etag = format!("\"{}\"", encode_hex(&sha256(&body)));
    let etag = HeaderValue::from_str(&etag).map_err(|_| ApiError::internal())?;
    let etag_text = etag.to_str().map_err(|_| ApiError::internal())?;
    let not_modified = request_headers.get_all(IF_NONE_MATCH).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|candidate| candidate.trim() == etag_text)
        })
    });
    let mut response = Response::new(if not_modified {
        Body::empty()
    } else {
        Body::from(body)
    });
    *response.status_mut() = if not_modified {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    response.headers_mut().insert(ETAG, etag);
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static(HISTORY_CACHE_CONTROL),
    );
    if !not_modified {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path as FsPath, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        body::to_bytes,
        http::{Request as HttpRequest, header::IF_NONE_MATCH},
        response::IntoResponse,
    };
    use rusqlite::Connection;
    use serde_json::{Value, json};

    use super::*;
    use crate::database::{
        Database, NewNodeRow, PingHistoryWriteRow, ResourceHistoryWriteRow, hydrate_startup,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestContext {
        state: AppState,
        path: PathBuf,
        public_id: String,
        node_id: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "monitor-history-http-{}-{id}.db",
                std::process::id()
            ));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open history test database");
            let public_id = "d".repeat(32);
            let node = database
                .create_node(
                    NewNodeRow {
                        public_id: public_id.clone(),
                        name: "History node".into(),
                        region_code: "US".into(),
                        traffic_limit_bytes: None,
                        traffic_reset_day: 1,
                        price_micros: None,
                        currency: None,
                        renewal_cycle: None,
                        expires_at: None,
                    },
                    [7; 32],
                    1,
                )
                .await
                .expect("create node");
            database.shutdown().await.expect("close fixture database");
            let connection = Connection::open(&path).expect("open target fixture");
            connection
                .execute(
                    "INSERT INTO ping_targets
                     (id, name, host, ip_family, enabled, sort_order, created_at, updated_at)
                     VALUES (1, 'Second', '127.0.0.1', 4, 1, 1, 1, 1),
                            (2, 'Disabled first', '::1', 6, 0, 0, 1, 1)",
                    [],
                )
                .expect("insert ping targets");
            drop(connection);
            let database = Database::open(&path).expect("reopen history test database");
            let now = unix_timestamp().expect("current timestamp");
            let bucket = now - now.rem_euclid(60) - 60;
            database
                .persist_history_batch(
                    vec![ResourceHistoryWriteRow {
                        node_id: node.id,
                        bucket_ts: bucket,
                        sample_count: 2,
                        cpu_usage_bp: 125,
                        load_1_milli: 100,
                        load_5_milli: 200,
                        load_15_milli: 300,
                        memory_used_bytes: 1_000,
                        swap_used_bytes: 10,
                        disk_used_bytes: 2_000,
                        rx_rate_bytes_per_sec: 30,
                        tx_rate_bytes_per_sec: 40,
                    }],
                    vec![PingHistoryWriteRow {
                        node_id: node.id,
                        bucket_ts: bucket,
                        target_id: 2,
                        sample_count: 2,
                        success_count: 1,
                        latency_avg_ms: Some(15.5),
                        latency_min_ms: Some(15.5),
                        latency_max_ms: Some(15.5),
                    }],
                )
                .await
                .expect("persist history fixtures");
            let hydration = hydrate_startup(&database).await.expect("hydrate state");
            Self {
                state: AppState::new(database, hydration),
                path,
                public_id,
                node_id: node.id,
            }
        }

        fn request(&self, query: &str) -> Request {
            HttpRequest::builder()
                .uri(format!("/?{query}"))
                .body(Body::empty())
                .expect("build request")
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

    #[test]
    fn query_range_is_strict() {
        assert_eq!(
            parse_range(Some("range=1h"))
                .unwrap_or_else(|_| panic!("1h"))
                .step,
            60
        );
        assert_eq!(
            parse_range(Some("range=7d"))
                .unwrap_or_else(|_| panic!("7d"))
                .step,
            300
        );
        for query in [
            None,
            Some(""),
            Some("range=2h"),
            Some("foo=1h"),
            Some("range=1h&range=6h"),
            Some("range=1h&x=1"),
        ] {
            assert!(parse_range(query).is_err());
        }
        for (query, expected_points) in [
            ("range=1h", 60),
            ("range=6h", 360),
            ("range=24h", 1_440),
            ("range=7d", 2_016),
        ] {
            let range = parse_range(Some(query)).unwrap_or_else(|_| panic!("valid range"));
            assert_eq!(
                aligned_timestamps(1_700_000_123 - range.duration, 1_700_000_123, range.step).len(),
                expected_points
            );
        }
    }

    #[test]
    fn resource_axis_is_dense_and_missing_buckets_are_null() {
        let series = dense_resource_series(
            61,
            241,
            60,
            vec![ResourceHistoryPoint {
                bucket_ts: 120,
                cpu_usage: 1.5,
                memory_used_bytes: 10,
                disk_used_bytes: 20,
                rx_rate_bytes_per_sec: 30,
                tx_rate_bytes_per_sec: 40,
            }],
        );
        assert_eq!(series.timestamp, vec![120, 180, 240]);
        assert_eq!(series.cpu, vec![Some(1.5), None, None]);
        assert_eq!(series.memory, vec![Some(10), None, None]);
    }

    #[test]
    fn ping_axis_and_target_order_follow_query_rows() {
        let (targets, timestamps, series) = dense_ping_series(
            0,
            600,
            300,
            vec![
                PingHistoryPoint {
                    target_id: 2,
                    name: "v6".into(),
                    ip_family: 6,
                    sort_order: 0,
                    bucket_ts: Some(0),
                    latency_ms: Some(20.0),
                },
                PingHistoryPoint {
                    target_id: 3,
                    name: "v4".into(),
                    ip_family: 4,
                    sort_order: 1,
                    bucket_ts: None,
                    latency_ms: None,
                },
            ],
        );
        assert_eq!(timestamps, vec![0, 300]);
        assert_eq!(
            targets.iter().map(|target| target.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(series[0].latency, vec![Some(20.0), None]);
        assert_eq!(series[1].latency, vec![None, None]);
    }

    #[tokio::test]
    async fn resource_http_contract_is_dense_cacheable_and_supports_etag() {
        let context = TestContext::new().await;
        let response = resource(
            State(context.state.clone()),
            Path(context.public_id.clone()),
            context.request("range=1h"),
        )
        .await
        .unwrap_or_else(IntoResponse::into_response);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], HISTORY_CACHE_CONTROL);
        let etag = response.headers()[ETAG].clone();
        assert!(
            etag.to_str()
                .is_ok_and(|value| value.starts_with('"') && value.ends_with('"'))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value: Value = serde_json::from_slice(&body).expect("parse history response");
        assert_eq!(value["node_id"], context.public_id);
        assert_eq!(value["step"], 60);
        assert_eq!(
            value["series"]["timestamp"].as_array().expect("axis").len(),
            60
        );
        for field in ["cpu", "memory", "disk", "rx_rate", "tx_rate"] {
            assert_eq!(value["series"][field].as_array().expect("series").len(), 60);
        }
        let serialized = String::from_utf8(body.to_vec()).expect("utf8 json");
        for forbidden in ["token", "boot_id", "last_ip", "internal_id", "password"] {
            assert!(!serialized.contains(forbidden));
        }

        let fixed_body = json!({"history": [1, null, 3]});
        let initial = history_response(&HeaderMap::new(), &fixed_body)
            .unwrap_or_else(IntoResponse::into_response);
        let etag = initial.headers()[ETAG].clone();
        let mut conditional_headers = HeaderMap::new();
        conditional_headers.insert(IF_NONE_MATCH, etag);
        let response = history_response(&conditional_headers, &fixed_body)
            .unwrap_or_else(IntoResponse::into_response);
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read 304 body")
                .is_empty()
        );
        let mut wrong = context.request("range=1h");
        wrong
            .headers_mut()
            .insert(IF_NONE_MATCH, HeaderValue::from_static("\"wrong\""));
        let response = resource(
            State(context.state.clone()),
            Path(context.public_id.clone()),
            wrong,
        )
        .await
        .unwrap_or_else(IntoResponse::into_response);
        assert_eq!(response.status(), StatusCode::OK);
        context.finish().await;
    }

    #[tokio::test]
    async fn ping_http_includes_disabled_history_and_sorted_targets() {
        let context = TestContext::new().await;
        let response = ping(
            State(context.state.clone()),
            Path(context.public_id.clone()),
            context.request("range=7d"),
        )
        .await
        .unwrap_or_else(IntoResponse::into_response);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], HISTORY_CACHE_CONTROL);
        let value: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read ping body"),
        )
        .expect("parse ping response");
        assert_eq!(value["step"], 300);
        assert_eq!(value["targets"][0]["id"], 2);
        assert_eq!(value["targets"][1]["id"], 1);
        assert_eq!(value["series"][0]["target_id"], 2);
        assert_eq!(value["timestamps"].as_array().expect("axis").len(), 2_016);
        context.finish().await;
    }

    #[tokio::test]
    async fn history_query_and_node_validation_errors_are_frozen() {
        let context = TestContext::new().await;
        for query in [
            "",
            "range=2h",
            "other=1h",
            "range=1h&range=6h",
            "range=1h&x=1",
        ] {
            let response = resource(
                State(context.state.clone()),
                Path(context.public_id.clone()),
                context.request(query),
            )
            .await
            .unwrap_or_else(IntoResponse::into_response);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }
        for id in ["bad".to_owned(), "A".repeat(32), "e".repeat(32)] {
            let response = resource(
                State(context.state.clone()),
                Path(id),
                context.request("range=6h"),
            )
            .await
            .unwrap_or_else(IntoResponse::into_response);
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        let response = resource(
            State(context.state.clone()),
            Path(context.public_id.clone()),
            context.request("range=24h"),
        )
        .await
        .unwrap_or_else(IntoResponse::into_response);
        let value: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read 24h body"),
        )
        .expect("parse 24h response");
        assert_eq!(
            value["series"]["timestamp"].as_array().expect("axis").len(),
            1_440
        );
        assert!(context.node_id > 0);
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
