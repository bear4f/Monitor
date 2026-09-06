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
pub struct TrafficRecoveryRow {
    pub node_id: i64,
    pub rx_total_bytes: i64,
    pub tx_total_bytes: i64,
    pub last_rx_counter_bytes: Option<i64>,
    pub last_tx_counter_bytes: Option<i64>,
    pub last_boot_id: Option<String>,
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
    pub traffic_recovery: Vec<TrafficRecoveryRow>,
}
