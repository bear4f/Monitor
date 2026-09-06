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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNodeRow {
    pub public_id: String,
    pub name: String,
    pub region_code: String,
    pub traffic_limit_bytes: Option<i64>,
    pub traffic_reset_day: i64,
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

#[derive(Debug)]
pub struct StartupHydration {
    pub settings: SettingsRow,
    pub nodes: Vec<NodeMetaRow>,
    pub node_tokens: Vec<NodeTokenRow>,
    pub node_last_states: Vec<NodeLastStateRow>,
    pub enabled_ping_targets: Vec<EnabledPingTargetRow>,
    pub traffic_recovery: Vec<TrafficRecoveryRow>,
}
