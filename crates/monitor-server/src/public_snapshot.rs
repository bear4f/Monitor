use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use axum::http::HeaderValue;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::{
    app::AppState,
    auth::{encode_hex, sha256, unix_timestamp},
    database::NodeMetaRow,
    snapshot::NodeSnapshot,
    time,
    traffic::{JS_SAFE_INTEGER_MAX, PublicTrafficState, browser_safe_counter},
};

const NETWORK_RATE_POINTS: usize = 60;
const GENERATION_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct PublicSnapshot {
    pub body: Arc<[u8]>,
    pub etag: HeaderValue,
    pub generated_at: i64,
}

#[derive(Clone)]
pub struct PublicSnapshotCache(Arc<RwLock<Arc<PublicSnapshot>>>);

impl PublicSnapshotCache {
    pub fn new() -> Self {
        let body: Arc<[u8]> = Arc::from(
            br#"{"generated_at":0,"site":{"name":"Monitor","timezone":"Asia/Shanghai","theme_default":"system"},"summary":{"online_nodes":0,"total_nodes":0,"busiest_node":null,"today_rx":0,"today_tx":0,"total_rx":0,"total_tx":0,"current_rx_rate":0,"current_tx_rate":0,"network_rate_history":{"timestamps":[],"rx":[],"tx":[]}},"nodes":[]}"#
                .as_slice(),
        );
        let etag = etag(&body);
        Self(Arc::new(RwLock::new(Arc::new(PublicSnapshot {
            body,
            etag,
            generated_at: 0,
        }))))
    }

    pub async fn load(&self) -> Arc<PublicSnapshot> {
        self.0.read().await.clone()
    }

    async fn publish(&self, snapshot: PublicSnapshot) {
        *self.0.write().await = Arc::new(snapshot);
    }
}

impl Default for PublicSnapshotCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkRatePoint {
    timestamp: i64,
    rx: i64,
    tx: i64,
}

#[derive(Clone, Default)]
pub struct NetworkRateRing(Arc<Mutex<VecDeque<NetworkRatePoint>>>);

impl NetworkRateRing {
    pub fn new() -> Self {
        Self::default()
    }

    fn candidate(&self, point: NetworkRatePoint) -> VecDeque<NetworkRatePoint> {
        let mut candidate = self.lock().clone();
        if candidate
            .back()
            .is_none_or(|previous| point.timestamp > previous.timestamp)
        {
            candidate.push_back(point);
            if candidate.len() > NETWORK_RATE_POINTS {
                candidate.pop_front();
            }
        }
        candidate
    }

    fn publish(&self, candidate: VecDeque<NetworkRatePoint>) {
        *self.lock() = candidate;
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<NetworkRatePoint>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub enum PublicSnapshotError {
    Clock,
    Time(jiff::Error),
    Serialize(serde_json::Error),
}

impl std::fmt::Display for PublicSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clock => formatter.write_str("system clock is outside the supported range"),
            Self::Time(error) => write!(formatter, "site time calculation failed: {error}"),
            Self::Serialize(error) => write!(formatter, "JSON serialization failed: {error}"),
        }
    }
}

impl std::error::Error for PublicSnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Time(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::Clock => None,
        }
    }
}

pub async fn generate_now(state: &AppState) -> Result<(), PublicSnapshotError> {
    let _generation_guard = state.public_snapshot_generation_gate.lock().await;
    let generated_at = unix_timestamp().map_err(|_| PublicSnapshotError::Clock)?;
    generate_locked(state, generated_at).await
}

#[cfg(test)]
pub async fn generate(state: &AppState, generated_at: i64) -> Result<(), PublicSnapshotError> {
    let _generation_guard = state.public_snapshot_generation_gate.lock().await;
    generate_locked(state, generated_at).await
}

async fn generate_locked(state: &AppState, generated_at: i64) -> Result<(), PublicSnapshotError> {
    let settings = state.settings.read().await.clone();
    let nodes: Vec<_> = state.node_metadata.read().await.values().cloned().collect();
    let snapshots = state.snapshots.read().await.clone();
    let traffic = state.traffic.read_current();
    let current_day_start_utc = time::day_start_utc(generated_at, &settings.site_timezone)
        .map_err(PublicSnapshotError::Time)?;

    let mut nodes = nodes;
    nodes.sort_by_key(|node| (node.sort_order, node.id));
    let mut public_nodes = Vec::with_capacity(nodes.len());
    let mut summary = SummaryAccumulator::default();

    for node in nodes {
        let snapshot = snapshots.get(&node.id);
        let online = snapshot.is_some_and(|snapshot| {
            snapshot.live_since_start
                && generated_at.saturating_sub(snapshot.last_seen_at)
                    <= settings.offline_after_seconds
        });
        let current_traffic = traffic.get(&node.id).copied();
        let node_response = public_node(
            &node,
            snapshot,
            current_traffic,
            online,
            generated_at,
            &settings.site_timezone,
            current_day_start_utc,
        )?;
        summary.add(&node_response);
        public_nodes.push(node_response);
    }

    let point = NetworkRatePoint {
        timestamp: generated_at,
        rx: summary.current_rx_rate,
        tx: summary.current_tx_rate,
    };
    let ring = state.public_network_rates.candidate(point);
    let network_rate_history = NetworkRateHistoryResponse::from(&ring);
    let response = PublicSnapshotResponse {
        generated_at,
        site: SiteResponse {
            name: settings.site_name,
            timezone: settings.site_timezone,
            theme_default: settings.theme_default,
        },
        summary: summary.finish(network_rate_history),
        nodes: public_nodes,
    };
    let body: Arc<[u8]> = serde_json::to_vec(&response)
        .map_err(PublicSnapshotError::Serialize)?
        .into();
    let etag = etag(&body);

    state.public_network_rates.publish(ring);
    state
        .public_snapshot
        .publish(PublicSnapshot {
            body,
            etag,
            generated_at,
        })
        .await;
    Ok(())
}

pub async fn run_worker(state: AppState) {
    let mut interval = tokio::time::interval(GENERATION_INTERVAL);
    interval.tick().await;
    loop {
        interval.tick().await;
        if let Err(error) = generate_now(&state).await {
            tracing::error!(error = %error, "public snapshot generation failed; previous cache retained");
        }
    }
}

#[derive(Serialize)]
struct PublicSnapshotResponse {
    generated_at: i64,
    site: SiteResponse,
    summary: SummaryResponse,
    nodes: Vec<PublicNodeResponse>,
}

#[derive(Serialize)]
struct SiteResponse {
    name: String,
    timezone: String,
    theme_default: String,
}

#[derive(Serialize)]
struct SummaryResponse {
    online_nodes: i64,
    total_nodes: i64,
    busiest_node: Option<BusiestNodeResponse>,
    today_rx: i64,
    today_tx: i64,
    total_rx: i64,
    total_tx: i64,
    current_rx_rate: i64,
    current_tx_rate: i64,
    network_rate_history: NetworkRateHistoryResponse,
}

#[derive(Serialize)]
struct BusiestNodeResponse {
    id: String,
    name: String,
    cpu_usage: f64,
}

#[derive(Serialize)]
struct NetworkRateHistoryResponse {
    timestamps: Vec<i64>,
    rx: Vec<i64>,
    tx: Vec<i64>,
}

impl From<&VecDeque<NetworkRatePoint>> for NetworkRateHistoryResponse {
    fn from(points: &VecDeque<NetworkRatePoint>) -> Self {
        let mut timestamps = Vec::with_capacity(points.len());
        let mut rx = Vec::with_capacity(points.len());
        let mut tx = Vec::with_capacity(points.len());
        for point in points {
            timestamps.push(point.timestamp);
            rx.push(point.rx);
            tx.push(point.tx);
        }
        Self { timestamps, rx, tx }
    }
}

#[derive(Serialize)]
struct PublicNodeResponse {
    id: String,
    name: String,
    region_code: String,
    sort_order: i64,
    online: bool,
    first_seen_at: Option<i64>,
    last_seen_at: Option<i64>,
    system: Option<SystemResponse>,
    metrics: Option<MetricsResponse>,
    traffic: TrafficResponse,
    billing: Option<BillingResponse>,
}

#[derive(Serialize)]
struct SystemResponse {
    hostname: String,
    os_name: String,
    os_version: String,
    kernel: String,
    architecture: String,
    virtualization: String,
    agent_version: String,
    cpu_model: String,
    cpu_cores: i64,
    process_count: i64,
    uptime_seconds: i64,
}

#[derive(Serialize)]
struct MetricsResponse {
    cpu_usage: f64,
    load_1: f64,
    load_5: f64,
    load_15: f64,
    memory_total: i64,
    memory_used: i64,
    swap_total: i64,
    swap_used: i64,
    disk_total: i64,
    disk_used: i64,
    current_rx_rate: i64,
    current_tx_rate: i64,
}

#[derive(Serialize)]
struct TrafficResponse {
    today_rx: i64,
    today_tx: i64,
    cycle_rx: i64,
    cycle_tx: i64,
    total_rx: i64,
    total_tx: i64,
    limit: Option<i64>,
    cycle_start_at: i64,
    cycle_end_at: i64,
}

#[derive(Serialize)]
struct BillingResponse {
    price_micros: i64,
    currency: String,
    renewal_cycle: Option<String>,
    expires_at: Option<i64>,
}

fn public_node(
    node: &NodeMetaRow,
    snapshot: Option<&NodeSnapshot>,
    traffic: Option<PublicTrafficState>,
    online: bool,
    generated_at: i64,
    timezone: &str,
    current_day_start_utc: i64,
) -> Result<PublicNodeResponse, PublicSnapshotError> {
    let current_cycle = time::billing_cycle(generated_at, timezone, node.traffic_reset_day)
        .map_err(PublicSnapshotError::Time)?;
    let total_rx = traffic.map_or(0, |traffic| browser_safe_counter(traffic.rx_total_bytes));
    let total_tx = traffic.map_or(0, |traffic| browser_safe_counter(traffic.tx_total_bytes));
    let (today_rx, today_tx) = traffic
        .filter(|traffic| traffic.day_start_utc == current_day_start_utc)
        .map_or((0, 0), |traffic| {
            (
                browser_safe_counter(traffic.today_rx_bytes),
                browser_safe_counter(traffic.today_tx_bytes),
            )
        });
    let (cycle_rx, cycle_tx) = traffic
        .filter(|traffic| {
            traffic.cycle_start_utc == current_cycle.start_utc
                && traffic.cycle_end_utc == current_cycle.end_utc
        })
        .map_or((0, 0), |traffic| {
            (
                browser_safe_counter(traffic.cycle_rx_bytes),
                browser_safe_counter(traffic.cycle_tx_bytes),
            )
        });
    Ok(PublicNodeResponse {
        id: node.public_id.clone(),
        name: node.name.clone(),
        region_code: node.region_code.clone(),
        sort_order: node.sort_order,
        online,
        first_seen_at: snapshot
            .map(|snapshot| snapshot.first_seen_at)
            .or(node.first_seen_at),
        last_seen_at: snapshot.map(|snapshot| snapshot.last_seen_at),
        system: snapshot.map(SystemResponse::from),
        metrics: snapshot.map(MetricsResponse::from),
        traffic: TrafficResponse {
            today_rx,
            today_tx,
            cycle_rx,
            cycle_tx,
            total_rx,
            total_tx,
            limit: node.traffic_limit_bytes,
            cycle_start_at: current_cycle.start_utc,
            cycle_end_at: current_cycle.end_utc,
        },
        billing: node.price_micros.map(|price_micros| BillingResponse {
            price_micros,
            currency: node
                .currency
                .clone()
                .expect("database enforces price and currency pairing"),
            renewal_cycle: node.renewal_cycle.clone(),
            expires_at: node.expires_at,
        }),
    })
}

impl From<&NodeSnapshot> for SystemResponse {
    fn from(snapshot: &NodeSnapshot) -> Self {
        Self {
            hostname: snapshot.hostname.clone(),
            os_name: snapshot.os_name.clone(),
            os_version: snapshot.os_version.clone(),
            kernel: snapshot.kernel.clone(),
            architecture: snapshot.architecture.clone(),
            virtualization: snapshot.virtualization.clone(),
            agent_version: snapshot.agent_version.clone(),
            cpu_model: snapshot.cpu_model.clone(),
            cpu_cores: snapshot.cpu_cores,
            process_count: snapshot.process_count,
            uptime_seconds: snapshot.uptime_seconds,
        }
    }
}

impl From<&NodeSnapshot> for MetricsResponse {
    fn from(snapshot: &NodeSnapshot) -> Self {
        Self {
            cpu_usage: snapshot.cpu_usage,
            load_1: snapshot.load_1,
            load_5: snapshot.load_5,
            load_15: snapshot.load_15,
            memory_total: snapshot.memory_total,
            memory_used: snapshot.memory_used,
            swap_total: snapshot.swap_total,
            swap_used: snapshot.swap_used,
            disk_total: snapshot.disk_total,
            disk_used: snapshot.disk_used,
            current_rx_rate: snapshot.rx_rate_bytes_per_sec,
            current_tx_rate: snapshot.tx_rate_bytes_per_sec,
        }
    }
}

#[derive(Default)]
struct SummaryAccumulator {
    online_nodes: i64,
    total_nodes: i64,
    busiest_node: Option<BusiestNodeResponse>,
    today_rx: i64,
    today_tx: i64,
    total_rx: i64,
    total_tx: i64,
    current_rx_rate: i64,
    current_tx_rate: i64,
}

impl SummaryAccumulator {
    fn add(&mut self, node: &PublicNodeResponse) {
        self.total_nodes = self.total_nodes.saturating_add(1);
        self.today_rx = browser_safe_add(self.today_rx, node.traffic.today_rx);
        self.today_tx = browser_safe_add(self.today_tx, node.traffic.today_tx);
        self.total_rx = browser_safe_add(self.total_rx, node.traffic.total_rx);
        self.total_tx = browser_safe_add(self.total_tx, node.traffic.total_tx);
        if !node.online {
            return;
        }
        self.online_nodes = self.online_nodes.saturating_add(1);
        if let Some(metrics) = &node.metrics {
            self.current_rx_rate = browser_safe_add(self.current_rx_rate, metrics.current_rx_rate);
            self.current_tx_rate = browser_safe_add(self.current_tx_rate, metrics.current_tx_rate);
            if self
                .busiest_node
                .as_ref()
                .is_none_or(|busiest| metrics.cpu_usage > busiest.cpu_usage)
            {
                self.busiest_node = Some(BusiestNodeResponse {
                    id: node.id.clone(),
                    name: node.name.clone(),
                    cpu_usage: metrics.cpu_usage,
                });
            }
        }
    }

    fn finish(self, network_rate_history: NetworkRateHistoryResponse) -> SummaryResponse {
        SummaryResponse {
            online_nodes: self.online_nodes,
            total_nodes: self.total_nodes,
            busiest_node: self.busiest_node,
            today_rx: self.today_rx,
            today_tx: self.today_tx,
            total_rx: self.total_rx,
            total_tx: self.total_tx,
            current_rx_rate: self.current_rx_rate,
            current_tx_rate: self.current_tx_rate,
            network_rate_history,
        }
    }
}

fn browser_safe_add(left: i64, right: i64) -> i64 {
    left.saturating_add(right).clamp(0, JS_SAFE_INTEGER_MAX)
}

fn etag(body: &[u8]) -> HeaderValue {
    let value = format!("\"{}\"", encode_hex(&sha256(body)));
    HeaderValue::from_str(&value).expect("SHA-256 ETag is a valid header value")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        net::{IpAddr, Ipv4Addr},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        body::{Body, to_bytes},
        extract::State,
        http::{Request, StatusCode, header::IF_NONE_MATCH},
    };
    use serde_json::Value;

    use super::*;
    use crate::{
        database::{Database, TrafficRecoveryRow, hydrate_startup},
        http,
        traffic::TrafficSample,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestContext {
        state: AppState,
        path: PathBuf,
    }

    impl TestContext {
        async fn new(
            label: &str,
            nodes: Vec<NodeMetaRow>,
            traffic: Vec<TrafficRecoveryRow>,
        ) -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "monitor-public-snapshot-{label}-{}-{id}.db",
                std::process::id()
            ));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open public snapshot test database");
            let mut hydration = hydrate_startup(&database)
                .await
                .expect("hydrate public snapshot test state");
            hydration.nodes = nodes;
            hydration.traffic_recovery = traffic;
            Self {
                state: AppState::new(database, hydration),
                path,
            }
        }

        async fn finish(self) {
            self.state
                .database
                .clone()
                .shutdown()
                .await
                .expect("shutdown public snapshot test database");
            remove_database_files(&self.path);
        }

        async fn stop_database(&self) {
            self.state
                .database
                .clone()
                .shutdown()
                .await
                .expect("stop public snapshot database worker");
        }
    }

    #[tokio::test]
    async fn initial_empty_snapshot_is_immediately_available_without_database() {
        let context = TestContext::new("initial", Vec::new(), Vec::new()).await;
        generate(&context.state, 1_000)
            .await
            .expect("initial snapshot");
        context.stop_database().await;

        let response = get(&context.state, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-cache");
        assert_eq!(response.headers()["content-type"], "application/json");
        assert!(response.headers().contains_key("etag"));
        let value = response_json(response).await;
        assert_eq!(value["generated_at"], 1_000);
        assert_eq!(value["summary"]["total_nodes"], 0);
        assert_eq!(value["summary"]["online_nodes"], 0);
        assert_eq!(value["nodes"], serde_json::json!([]));
        for _ in 0..4 {
            assert_eq!(get(&context.state, None).await.status(), StatusCode::OK);
        }

        remove_database_files(&context.path);
    }

    #[tokio::test]
    async fn handler_reuses_cached_bytes_and_honors_strong_etag() {
        let context = TestContext::new("etag", vec![node(1, 0)], Vec::new()).await;
        generate(&context.state, 2_000).await.expect("snapshot");
        let cached = context.state.public_snapshot.load().await;
        let expected = cached.body.as_ref().to_vec();
        let etag = cached.etag.to_str().expect("ETag text").to_owned();

        context.state.node_metadata.write().await.clear();
        for _ in 0..100 {
            let response = get(&context.state, None).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_bytes(response).await, expected);
        }

        let response = get(&context.state, Some(&etag)).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()["etag"], etag);
        assert_eq!(response.headers()["cache-control"], "no-cache");
        assert!(response_bytes(response).await.is_empty());

        let response = get(&context.state, Some("\"wrong\"")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_bytes(response).await, expected);
        context.finish().await;
    }

    #[tokio::test]
    async fn nodes_are_sorted_without_exposing_private_state_and_never_reported_is_null() {
        let mut later = node(20, 1);
        later.price_micros = Some(39_900_000);
        later.currency = Some("USD".into());
        later.renewal_cycle = Some("annual".into());
        later.expires_at = Some(1_800_000_000);
        let earlier = node(10, 0);
        let context = TestContext::new("sorting", vec![later, earlier], Vec::new()).await;
        generate(&context.state, 1_788_664_402)
            .await
            .expect("sorted snapshot");
        let value = cached_json(&context.state).await;
        assert_eq!(value["nodes"][0]["id"], public_id(10));
        assert_eq!(value["nodes"][1]["id"], public_id(20));
        assert_eq!(value["nodes"][0]["sort_order"], 0);
        assert_eq!(value["nodes"][1]["sort_order"], 1);
        let first = &value["nodes"][0];
        assert_eq!(first["online"], false);
        assert!(first["first_seen_at"].is_null());
        assert!(first["last_seen_at"].is_null());
        assert!(first["system"].is_null());
        assert!(first["metrics"].is_null());
        assert_eq!(first["traffic"]["total_rx"], 0);
        let cycle = time::billing_cycle(1_788_664_402, "Asia/Shanghai", 1).expect("expected cycle");
        assert_eq!(first["traffic"]["cycle_start_at"], cycle.start_utc);
        assert_eq!(first["traffic"]["cycle_end_at"], cycle.end_utc);
        assert!(first["billing"].is_null());
        assert_eq!(value["nodes"][1]["billing"]["currency"], "USD");

        for forbidden in [
            "last_ip",
            "token",
            "token_hash",
            "node_id",
            "boot_id",
            "rx_counter_bytes",
            "tx_counter_bytes",
        ] {
            assert!(!has_key_recursive(&value, forbidden), "leaked {forbidden}");
        }
        context.finish().await;
    }

    #[tokio::test]
    async fn online_rates_and_busiest_use_live_snapshots_and_stable_ties() {
        let context = TestContext::new(
            "online",
            vec![node(3, 2), node(2, 1), node(1, 0)],
            Vec::new(),
        )
        .await;
        {
            let mut snapshots = context.state.snapshots.write().await;
            snapshots.insert(1, snapshot(990, 50.0, 10, 20));
            snapshots.insert(2, snapshot(991, 50.0, 30, 40));
            snapshots.insert(3, snapshot(989, 99.0, 1_000, 2_000));
        }
        {
            let mut settings = context.state.settings.write().await;
            settings.offline_after_seconds = 10;
        }
        generate(&context.state, 1_000)
            .await
            .expect("online snapshot");
        let value = cached_json(&context.state).await;
        assert_eq!(value["summary"]["online_nodes"], 2);
        assert_eq!(value["summary"]["current_rx_rate"], 40);
        assert_eq!(value["summary"]["current_tx_rate"], 60);
        assert_eq!(value["summary"]["busiest_node"]["id"], public_id(1));
        assert_eq!(value["nodes"][2]["online"], false);
        assert_eq!(value["nodes"][2]["metrics"]["cpu_usage"], 99.0);

        context.state.snapshots.write().await.clear();
        generate(&context.state, 1_002)
            .await
            .expect("all offline snapshot");
        let value = cached_json(&context.state).await;
        assert!(value["summary"]["busiest_node"].is_null());
        context.finish().await;
    }

    #[tokio::test]
    async fn realtime_uncheckpointed_traffic_drives_node_and_summary_values() {
        let context = TestContext::new("realtime-traffic", vec![node(1, 0)], Vec::new()).await;
        let cycle = time::billing_cycle(5_000, "Asia/Shanghai", 1).expect("cycle");
        let day = time::day_start_utc(5_000, "Asia/Shanghai").expect("day");
        context
            .state
            .traffic
            .update(1, traffic_sample(100, 200, day, cycle))
            .expect("baseline");
        context
            .state
            .traffic
            .update(1, traffic_sample(150, 275, day, cycle))
            .expect("uncheckpointed delta");
        generate(&context.state, 5_000)
            .await
            .expect("traffic snapshot");
        let value = cached_json(&context.state).await;
        for field in ["today_rx", "cycle_rx", "total_rx"] {
            assert_eq!(value["nodes"][0]["traffic"][field], 50);
        }
        for field in ["today_tx", "cycle_tx", "total_tx"] {
            assert_eq!(value["nodes"][0]["traffic"][field], 75);
        }
        assert_eq!(value["summary"]["today_rx"], 50);
        assert_eq!(value["summary"]["today_tx"], 75);
        assert_eq!(value["summary"]["total_rx"], 50);
        assert_eq!(value["summary"]["total_tx"], 75);
        context.finish().await;
    }

    #[tokio::test]
    async fn recovered_traffic_is_public_while_live_status_starts_offline() {
        let generated_at = 8_000;
        let context = TestContext::new(
            "recovered",
            vec![node(1, 0)],
            vec![recovery(
                1,
                (500, 600),
                (70, 80),
                (90, 100),
                generated_at,
                1,
            )],
        )
        .await;
        generate(&context.state, generated_at)
            .await
            .expect("recovered snapshot");
        let value = cached_json(&context.state).await;
        assert_eq!(value["nodes"][0]["online"], false);
        assert!(value["nodes"][0]["metrics"].is_null());
        assert_eq!(value["nodes"][0]["traffic"]["total_rx"], 500);
        assert_eq!(value["nodes"][0]["traffic"]["today_tx"], 80);
        assert_eq!(value["nodes"][0]["traffic"]["cycle_rx"], 90);
        assert_eq!(value["summary"]["total_tx"], 600);
        context.finish().await;
    }

    #[tokio::test]
    async fn network_rate_ring_is_bounded_aligned_and_strictly_increasing() {
        let context = TestContext::new("ring", Vec::new(), Vec::new()).await;
        for timestamp in 1..=61 {
            generate(&context.state, timestamp)
                .await
                .expect("ring snapshot");
        }
        generate(&context.state, 61)
            .await
            .expect("duplicate timestamp snapshot");
        let value = cached_json(&context.state).await;
        let history = &value["summary"]["network_rate_history"];
        let timestamps = history["timestamps"].as_array().expect("timestamps");
        let rx = history["rx"].as_array().expect("RX history");
        let tx = history["tx"].as_array().expect("TX history");
        assert_eq!(timestamps.len(), 60);
        assert_eq!(rx.len(), timestamps.len());
        assert_eq!(tx.len(), timestamps.len());
        assert_eq!(timestamps.first(), Some(&Value::from(2)));
        assert_eq!(timestamps.last(), Some(&Value::from(61)));
        assert!(
            timestamps
                .windows(2)
                .all(|pair| pair[0].as_i64() < pair[1].as_i64())
        );
        context.finish().await;
    }

    #[tokio::test]
    async fn summary_aggregate_saturates_at_the_browser_safe_boundary() {
        let generated_at = 9_000;
        let context = TestContext::new(
            "overflow",
            vec![node(1, 0), node(2, 1)],
            vec![recovery(
                1,
                (JS_SAFE_INTEGER_MAX + 1_000, JS_SAFE_INTEGER_MAX + 2_000),
                (JS_SAFE_INTEGER_MAX + 3_000, JS_SAFE_INTEGER_MAX + 4_000),
                (JS_SAFE_INTEGER_MAX + 5_000, JS_SAFE_INTEGER_MAX + 6_000),
                generated_at,
                1,
            )],
        )
        .await;
        generate(&context.state, generated_at)
            .await
            .expect("last valid snapshot");
        let internal = context
            .state
            .traffic
            .read_current()
            .get(&1)
            .copied()
            .expect("internal traffic state");
        assert_eq!(internal.rx_total_bytes, JS_SAFE_INTEGER_MAX + 1_000);
        assert_eq!(internal.today_rx_bytes, JS_SAFE_INTEGER_MAX + 3_000);
        assert_eq!(internal.cycle_rx_bytes, JS_SAFE_INTEGER_MAX + 5_000);
        let first = cached_json(&context.state).await;
        assert_eq!(
            first["nodes"][0]["traffic"]["total_rx"],
            JS_SAFE_INTEGER_MAX
        );
        assert_eq!(
            first["nodes"][0]["traffic"]["today_rx"],
            JS_SAFE_INTEGER_MAX
        );
        assert_eq!(
            first["nodes"][0]["traffic"]["cycle_rx"],
            JS_SAFE_INTEGER_MAX
        );
        let cycle = time::billing_cycle(9_001, "Asia/Shanghai", 1).expect("cycle");
        let day = time::day_start_utc(9_001, "Asia/Shanghai").expect("day");
        context
            .state
            .traffic
            .update(2, traffic_sample(0, 0, day, cycle))
            .expect("second node baseline");
        context
            .state
            .traffic
            .update(2, traffic_sample(JS_SAFE_INTEGER_MAX, 0, day, cycle))
            .expect("second node traffic");

        generate(&context.state, 9_001)
            .await
            .expect("saturated summary snapshot");
        let current = cached_json(&context.state).await;
        assert_eq!(current["summary"]["total_rx"], JS_SAFE_INTEGER_MAX);
        assert_eq!(current["summary"]["today_rx"], JS_SAFE_INTEGER_MAX);
        assert_eq!(
            current["nodes"][0]["traffic"]["total_rx"],
            JS_SAFE_INTEGER_MAX
        );
        assert_eq!(
            current["nodes"][1]["traffic"]["total_rx"],
            JS_SAFE_INTEGER_MAX
        );
        assert_eq!(context.state.public_network_rates.lock().len(), 2);
        context.finish().await;
    }

    #[tokio::test]
    async fn public_view_normalizes_stale_and_current_natural_day_buckets() {
        let generated_at = second("2026-09-07T12:00:00Z");
        let yesterday = second("2026-09-06T12:00:00Z");
        let context = TestContext::new(
            "day-normalization",
            vec![node(1, 0), node(2, 1)],
            vec![
                recovery(1, (500, 600), (100, 200), (10, 20), yesterday, 1),
                recovery(2, (700, 800), (300, 400), (30, 40), generated_at, 1),
            ],
        )
        .await;
        generate(&context.state, generated_at)
            .await
            .expect("normalized daily snapshot");
        let value = cached_json(&context.state).await;
        assert_eq!(value["nodes"][0]["traffic"]["today_rx"], 0);
        assert_eq!(value["nodes"][0]["traffic"]["today_tx"], 0);
        assert_eq!(value["nodes"][0]["traffic"]["total_rx"], 500);
        assert_eq!(value["nodes"][0]["traffic"]["total_tx"], 600);
        assert_eq!(value["nodes"][1]["traffic"]["today_rx"], 300);
        assert_eq!(value["nodes"][1]["traffic"]["today_tx"], 400);
        assert_eq!(value["summary"]["today_rx"], 300);
        assert_eq!(value["summary"]["today_tx"], 400);
        context.finish().await;
    }

    #[tokio::test]
    async fn public_view_normalizes_stale_cycle_with_reset_day_31_across_february() {
        let generated_at = second("2026-03-01T12:00:00Z");
        let previous_cycle_time = second("2026-02-20T12:00:00Z");
        let mut metadata = node(1, 0);
        metadata.traffic_reset_day = 31;
        let context = TestContext::new(
            "cycle-normalization",
            vec![metadata],
            vec![recovery(
                1,
                (500, 600),
                (100, 200),
                (300, 400),
                previous_cycle_time,
                31,
            )],
        )
        .await;
        generate(&context.state, generated_at)
            .await
            .expect("normalized cycle snapshot");
        let value = cached_json(&context.state).await;
        let traffic = &value["nodes"][0]["traffic"];
        let expected = time::billing_cycle(generated_at, "Asia/Shanghai", 31)
            .expect("current reset-day-31 cycle");
        assert_eq!(traffic["cycle_rx"], 0);
        assert_eq!(traffic["cycle_tx"], 0);
        assert_eq!(traffic["cycle_start_at"], expected.start_utc);
        assert_eq!(traffic["cycle_end_at"], expected.end_utc);
        assert_eq!(traffic["total_rx"], 500);
        assert_eq!(traffic["total_tx"], 600);
        context.finish().await;
    }

    async fn get(state: &AppState, etag: Option<&str>) -> axum::response::Response {
        let mut builder = Request::builder().uri("/api/public/snapshot");
        if let Some(etag) = etag {
            builder = builder.header(IF_NONE_MATCH, etag);
        }
        http::public::snapshot(
            State(state.clone()),
            builder.body(Body::empty()).expect("public request"),
        )
        .await
    }

    async fn response_bytes(response: axum::response::Response) -> Vec<u8> {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read public response")
            .to_vec()
    }

    async fn response_json(response: axum::response::Response) -> Value {
        serde_json::from_slice(&response_bytes(response).await).expect("public response JSON")
    }

    async fn cached_json(state: &AppState) -> Value {
        let snapshot = state.public_snapshot.load().await;
        serde_json::from_slice(&snapshot.body).expect("cached public JSON")
    }

    fn node(id: i64, sort_order: i64) -> NodeMetaRow {
        NodeMetaRow {
            id,
            public_id: public_id(id),
            name: format!("Node {id}"),
            region_code: "US".into(),
            sort_order,
            traffic_limit_bytes: None,
            traffic_reset_day: 1,
            price_micros: None,
            currency: None,
            renewal_cycle: None,
            expires_at: None,
            first_seen_at: None,
        }
    }

    fn public_id(id: i64) -> String {
        format!("{id:032x}")
    }

    fn snapshot(last_seen_at: i64, cpu_usage: f64, rx_rate: i64, tx_rate: i64) -> NodeSnapshot {
        NodeSnapshot {
            live_since_start: true,
            first_seen_at: last_seen_at - 100,
            last_seen_at,
            last_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            hostname: "node.example".into(),
            os_name: "Debian".into(),
            os_version: "13".into(),
            kernel: "6.12.0".into(),
            architecture: "x86_64".into(),
            virtualization: "qemu".into(),
            agent_version: "0.1.0".into(),
            cpu_model: "Test CPU".into(),
            cpu_cores: 1,
            cpu_usage,
            load_1: 0.1,
            load_5: 0.2,
            load_15: 0.3,
            memory_total: 1_000,
            memory_used: 500,
            swap_total: 100,
            swap_used: 10,
            disk_total: 10_000,
            disk_used: 2_000,
            rx_counter_bytes: 1_000,
            tx_counter_bytes: 2_000,
            rx_rate_bytes_per_sec: rx_rate,
            tx_rate_bytes_per_sec: tx_rate,
            uptime_seconds: 1_000,
            process_count: 50,
            boot_id: "private-boot-id".into(),
        }
    }

    fn recovery(
        node_id: i64,
        total: (i64, i64),
        today: (i64, i64),
        cycle_usage: (i64, i64),
        timestamp: i64,
        reset_day: i64,
    ) -> TrafficRecoveryRow {
        let day_start_utc = time::day_start_utc(timestamp, "Asia/Shanghai").expect("recovery day");
        let cycle = time::billing_cycle(timestamp, "Asia/Shanghai", reset_day)
            .expect("recovery billing cycle");
        TrafficRecoveryRow {
            node_id,
            rx_total_bytes: total.0,
            tx_total_bytes: total.1,
            last_rx_counter_bytes: None,
            last_tx_counter_bytes: None,
            last_boot_id: None,
            day_start_utc,
            today_rx_bytes: today.0,
            today_tx_bytes: today.1,
            cycle_start_utc: cycle.start_utc,
            cycle_end_utc: cycle.end_utc,
            cycle_rx_bytes: cycle_usage.0,
            cycle_tx_bytes: cycle_usage.1,
        }
    }

    fn second(value: &str) -> i64 {
        value
            .parse::<jiff::Timestamp>()
            .expect("valid timestamp")
            .as_second()
    }

    fn traffic_sample(
        rx: i64,
        tx: i64,
        day_start_utc: i64,
        billing_cycle: time::BillingCycle,
    ) -> TrafficSample<'static> {
        TrafficSample {
            rx_counter_bytes: rx,
            tx_counter_bytes: tx,
            boot_id: "boot-a",
            day_start_utc,
            billing_cycle,
        }
    }

    fn has_key_recursive(value: &Value, name: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key(name)
                    || object.values().any(|value| has_key_recursive(value, name))
            }
            Value::Array(array) => array.iter().any(|value| has_key_recursive(value, name)),
            _ => false,
        }
    }

    fn remove_database_files(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove public snapshot test database: {error}"),
            }
        }
    }
}
