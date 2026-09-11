#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRow {
    pub site_name: String,
    pub site_timezone: String,
    pub theme_default: String,
    pub history_retention_days: i64,
    pub agent_report_interval_seconds: i64,
    pub ping_interval_seconds: i64,
    pub offline_after_seconds: i64,
    pub default_traffic_reset_day: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMetaRow {
    pub id: i64,
    pub public_id: String,
    pub name: String,
    pub region_code: String,
    pub sort_order: i64,
    pub traffic_limit_bytes: Option<i64>,
    pub traffic_reset_day: i64,
    pub traffic_reset_mode: String,
    pub price_micros: Option<i64>,
    pub currency: Option<String>,
    pub renewal_cycle: Option<String>,
    pub expires_at: Option<i64>,
    pub first_seen_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTokenRow {
    pub node_id: i64,
    pub token_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeLastStateRow {
    pub node_id: i64,
    pub snapshot: crate::snapshot::NodeSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledPingTargetRow {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub ip_family: i64,
    pub probe_kind: String,
    pub port: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingTargetRow {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub ip_family: i64,
    pub probe_kind: String,
    pub port: Option<i64>,
    pub enabled: bool,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPingTargetRow {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub ip_family: i64,
    pub probe_kind: String,
    pub port: Option<i64>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PingTargetPatchRow {
    pub name: Option<String>,
    /// Raw endpoint text. Parsing needs the final probe kind, so it happens where
    /// the existing row is known rather than field by field.
    pub target: Option<String>,
    pub probe_kind: Option<String>,
    pub ip_family: Option<i64>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingTargetMutationRow {
    pub target: PingTargetRow,
    pub enabled_targets: Vec<EnabledPingTargetRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatePingTargetResult {
    IdCollision,
    Conflict,
    Created(PingTargetMutationRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePingTargetResult {
    NotFound,
    InvalidSortOrder,
    InvalidConfiguration,
    Conflict,
    Updated(PingTargetMutationRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedPingTargetRow {
    pub target_id: i64,
    pub enabled_targets: Vec<EnabledPingTargetRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNodeRow {
    pub public_id: String,
    pub name: String,
    pub region_code: String,
    pub traffic_limit_bytes: Option<i64>,
    pub traffic_reset_day: i64,
    pub traffic_reset_mode: String,
    pub price_micros: Option<i64>,
    pub currency: Option<String>,
    pub renewal_cycle: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodePatchRow {
    pub name: Option<String>,
    pub region_code: Option<String>,
    pub traffic_limit_bytes: Option<Option<i64>>,
    pub traffic_reset_day: Option<i64>,
    pub traffic_reset_mode: Option<String>,
    pub price_micros: Option<Option<i64>>,
    pub currency: Option<Option<String>>,
    pub renewal_cycle: Option<Option<String>>,
    pub expires_at: Option<Option<i64>>,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeUpdateResult {
    pub node: NodeMetaRow,
    pub reordered_nodes: Vec<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateNodeResult {
    NotFound,
    InvalidSortOrder,
    InvalidConfiguration,
    Updated(Box<NodeUpdateResult>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedNodeRow {
    pub node_id: i64,
    pub token_hash: [u8; 32],
    pub reordered_nodes: Vec<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotatedNodeTokenRow {
    pub node_id: i64,
    pub old_token_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminNodeRow {
    pub node: NodeMetaRow,
    pub last_ip: Option<String>,
    pub last_seen_at: Option<i64>,
    pub total_rx_bytes: i64,
    pub total_tx_bytes: i64,
    pub today_rx_bytes: i64,
    pub today_tx_bytes: i64,
    pub cycle_rx_bytes: i64,
    pub cycle_tx_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficRecoveryRow {
    pub node_id: i64,
    pub rx_total_bytes: i64,
    pub tx_total_bytes: i64,
    pub last_rx_counter_bytes: Option<i64>,
    pub last_tx_counter_bytes: Option<i64>,
    pub last_boot_id: Option<String>,
    pub day_start_utc: i64,
    pub today_rx_bytes: i64,
    pub today_tx_bytes: i64,
    pub cycle_start_utc: i64,
    pub cycle_end_utc: i64,
    pub cycle_rx_bytes: i64,
    pub cycle_tx_bytes: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrafficCheckpointRow {
    pub node_id: i64,
    pub captured_generation: u64,
    pub rx_total_bytes: i64,
    pub tx_total_bytes: i64,
    pub last_rx_counter_bytes: i64,
    pub last_tx_counter_bytes: i64,
    pub last_boot_id: String,
    pub day_start_utc: i64,
    pub today_rx_bytes: i64,
    pub today_tx_bytes: i64,
    pub cycle_start_utc: i64,
    pub cycle_end_utc: i64,
    pub cycle_rx_bytes: i64,
    pub cycle_tx_bytes: i64,
    pub previous_day: Option<TrafficDayCheckpointRow>,
    pub previous_cycle: Option<TrafficCycleCheckpointRow>,
    pub snapshot: crate::snapshot::NodeSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficDayCheckpointRow {
    pub day_start_utc: i64,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficCycleCheckpointRow {
    pub cycle_start_utc: i64,
    pub cycle_end_utc: i64,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceHistoryWriteRow {
    pub node_id: i64,
    pub bucket_ts: i64,
    pub sample_count: i64,
    pub cpu_usage_bp: i64,
    pub load_1_milli: i64,
    pub load_5_milli: i64,
    pub load_15_milli: i64,
    pub memory_used_bytes: i64,
    pub swap_used_bytes: i64,
    pub disk_used_bytes: i64,
    pub rx_rate_bytes_per_sec: i64,
    pub tx_rate_bytes_per_sec: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PingHistoryWriteRow {
    pub node_id: i64,
    pub bucket_ts: i64,
    pub target_id: i64,
    pub sample_count: i64,
    pub success_count: i64,
    pub latency_avg_ms: Option<f64>,
    pub latency_min_ms: Option<f64>,
    pub latency_max_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceHistoryPoint {
    pub bucket_ts: i64,
    pub cpu_usage: f64,
    pub memory_used_bytes: i64,
    pub disk_used_bytes: i64,
    pub rx_rate_bytes_per_sec: i64,
    pub tx_rate_bytes_per_sec: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PingHistoryPoint {
    pub target_id: i64,
    pub name: String,
    pub ip_family: i64,
    pub sort_order: i64,
    /// `None` on every field below means the LEFT JOIN found no bucket for this
    /// target in the window, which is not the same as a bucket holding no
    /// samples -- packet loss is unknown in the first case and defined in the
    /// second, so the two must stay distinguishable.
    pub bucket_ts: Option<i64>,
    pub sample_count: Option<i64>,
    pub success_count: Option<i64>,
    pub latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRow {
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlitePragmas {
    pub foreign_keys: i64,
    pub journal_mode: String,
    pub busy_timeout_ms: i64,
    pub synchronous: i64,
    pub temp_store: i64,
    pub cache_size: i64,
    pub wal_autocheckpoint: i64,
    pub page_size: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaintenanceCleanupResult {
    pub expired_sessions: usize,
    pub traffic_daily: usize,
    pub traffic_cycles: usize,
}

impl MaintenanceCleanupResult {
    pub fn total(self) -> usize {
        self.expired_sessions + self.traffic_daily + self.traffic_cycles
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpointResult {
    pub busy: i64,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

#[derive(Debug)]
pub struct StartupHydration {
    pub settings: SettingsRow,
    pub nodes: Vec<NodeMetaRow>,
    pub node_tokens: Vec<NodeTokenRow>,
    pub node_last_states: Vec<NodeLastStateRow>,
    pub enabled_ping_targets: Vec<EnabledPingTargetRow>,
    pub traffic_recovery: Vec<TrafficRecoveryRow>,
}
