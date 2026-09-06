//! Wire types shared by the Monitor server and Linux agent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentReport {
    pub protocol_version: i64,
    pub agent_version: String,
    pub boot_id: String,
    pub hostname: String,
    pub os: OsReport,
    pub cpu: CpuReport,
    pub memory: MemoryReport,
    pub disk: DiskReport,
    pub network: NetworkReport,
    pub uptime_seconds: i64,
    pub process_count: i64,
    pub pings: Vec<PingReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OsReport {
    pub name: String,
    pub version: String,
    pub kernel: String,
    pub architecture: String,
    pub virtualization: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuReport {
    pub model: String,
    pub cores: i64,
    pub usage: f64,
    pub load_1: f64,
    pub load_5: f64,
    pub load_15: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReport {
    pub total: i64,
    pub used: i64,
    pub swap_total: i64,
    pub swap_used: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskReport {
    pub total: i64,
    pub used: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkReport {
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub rx_rate: i64,
    pub tx_rate: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PingReport {
    pub target_id: i64,
    pub success: bool,
    pub latency_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigPayload {
    pub protocol_version: i64,
    pub report_interval_seconds: i64,
    pub ping_interval_seconds: i64,
    pub targets: Vec<AgentPingTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPingTarget {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub ip_family: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> AgentReport {
        AgentReport {
            protocol_version: 1,
            agent_version: "0.1.0".to_owned(),
            boot_id: "boot-id".to_owned(),
            hostname: "node-1".to_owned(),
            os: OsReport {
                name: "Debian".to_owned(),
                version: "13".to_owned(),
                kernel: "6.12".to_owned(),
                architecture: "x86_64".to_owned(),
                virtualization: "kvm".to_owned(),
            },
            cpu: CpuReport {
                model: "Example CPU".to_owned(),
                cores: 2,
                usage: 12.5,
                load_1: 0.1,
                load_5: 0.2,
                load_15: 0.3,
            },
            memory: MemoryReport {
                total: 1024,
                used: 512,
                swap_total: 256,
                swap_used: 64,
            },
            disk: DiskReport {
                total: 4096,
                used: 1024,
            },
            network: NetworkReport {
                rx_bytes: 10,
                tx_bytes: 20,
                rx_rate: 1,
                tx_rate: 2,
            },
            uptime_seconds: 60,
            process_count: 10,
            pings: vec![PingReport {
                target_id: 1,
                success: false,
                latency_ms: None,
            }],
        }
    }

    #[test]
    fn agent_report_json_round_trip_preserves_wire_contract() {
        let value = serde_json::to_value(report()).expect("serialize report");
        assert_eq!(value["network"]["rx_rate"], 1);
        assert!(value.get("timestamp").is_none());
        let decoded: AgentReport = serde_json::from_value(value).expect("deserialize report");
        assert_eq!(decoded, report());
    }

    #[test]
    fn agent_config_json_round_trip_preserves_wire_contract() {
        let config = AgentConfigPayload {
            protocol_version: 1,
            report_interval_seconds: 2,
            ping_interval_seconds: 15,
            targets: vec![AgentPingTarget {
                id: 1,
                name: "Carrier v4".to_owned(),
                host: "203.0.113.1".to_owned(),
                ip_family: 4,
            }],
        };
        let json = serde_json::to_string(&config).expect("serialize config");
        assert_eq!(
            serde_json::from_str::<AgentConfigPayload>(&json).expect("deserialize config"),
            config
        );
    }

    #[test]
    fn unknown_nested_report_field_is_rejected() {
        let mut value = serde_json::to_value(report()).expect("serialize report");
        value["cpu"]["temperature"] = serde_json::json!(40);
        assert!(serde_json::from_value::<AgentReport>(value).is_err());
    }
}
