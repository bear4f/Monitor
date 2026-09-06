use std::{collections::HashMap, net::IpAddr, sync::Arc};

use tokio::sync::RwLock;

pub type SnapshotStore = Arc<RwLock<HashMap<i64, NodeSnapshot>>>;

#[derive(Debug, Clone, PartialEq)]
pub struct NodeSnapshot {
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub last_ip: IpAddr,
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel: String,
    pub architecture: String,
    pub virtualization: String,
    pub agent_version: String,
    pub cpu_model: String,
    pub cpu_cores: i64,
    pub cpu_usage: f64,
    pub load_1: f64,
    pub load_5: f64,
    pub load_15: f64,
    pub memory_total: i64,
    pub memory_used: i64,
    pub swap_total: i64,
    pub swap_used: i64,
    pub disk_total: i64,
    pub disk_used: i64,
    pub rx_counter_bytes: i64,
    pub tx_counter_bytes: i64,
    pub rx_rate_bytes_per_sec: i64,
    pub tx_rate_bytes_per_sec: i64,
    pub uptime_seconds: i64,
    pub process_count: i64,
    pub boot_id: String,
}
