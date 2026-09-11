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

/// Config protocol 1 payload. Frozen for the same reason as its target type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigPayload {
    pub protocol_version: i64,
    pub report_interval_seconds: i64,
    pub ping_interval_seconds: i64,
    pub targets: Vec<AgentPingTarget>,
}

/// Config protocol 2 payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigPayloadV2 {
    pub protocol_version: i64,
    pub report_interval_seconds: i64,
    pub ping_interval_seconds: i64,
    pub targets: Vec<AgentPingTargetV2>,
}

/// Reads only the version, ignoring everything else, so the body can then be
/// parsed strictly as the version it declares.
#[derive(Debug, Deserialize)]
struct ConfigVersionProbe {
    protocol_version: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigParseError {
    Malformed,
    UnsupportedVersion(i64),
    InvalidEndpoint,
}

impl AgentConfigPayloadV2 {
    /// Parses a configuration body of either protocol version into the richer
    /// shape. The declared version decides which struct is used, rather than
    /// trying one and falling back: a protocol 2 body with a bad endpoint must be
    /// rejected, not silently reinterpreted as protocol 1.
    pub fn from_json(body: &str) -> Result<Self, ConfigParseError> {
        let probe: ConfigVersionProbe =
            serde_json::from_str(body).map_err(|_| ConfigParseError::Malformed)?;
        match probe.protocol_version {
            CONFIG_VERSION_V1 => {
                let legacy: AgentConfigPayload =
                    serde_json::from_str(body).map_err(|_| ConfigParseError::Malformed)?;
                Ok(Self {
                    protocol_version: legacy.protocol_version,
                    report_interval_seconds: legacy.report_interval_seconds,
                    ping_interval_seconds: legacy.ping_interval_seconds,
                    targets: legacy
                        .targets
                        .into_iter()
                        .map(AgentPingTargetV2::from_legacy)
                        .collect(),
                })
            }
            CONFIG_VERSION_V2 => {
                let payload: Self =
                    serde_json::from_str(body).map_err(|_| ConfigParseError::Malformed)?;
                if payload
                    .targets
                    .iter()
                    .any(|target| !target.endpoint_is_valid())
                {
                    return Err(ConfigParseError::InvalidEndpoint);
                }
                Ok(payload)
            }
            other => Err(ConfigParseError::UnsupportedVersion(other)),
        }
    }
}

/// Config protocol 1 target. Frozen: a v0.1.2 Agent parses this with
/// `deny_unknown_fields`, so a single added field would make it reject the whole
/// configuration. Nothing may be added here -- config protocol 2 carries the
/// probe fields instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPingTarget {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub ip_family: i64,
}

/// Header a config-protocol-2 Agent sends on `GET /api/agent/config`. A Server
/// that does not know it ignores it and answers protocol 1, which is what makes
/// a new Agent work against an old Server.
pub const CONFIG_VERSION_HEADER: &str = "x-monitor-config-version";
pub const CONFIG_VERSION_V1: i64 = 1;
pub const CONFIG_VERSION_V2: i64 = 2;

/// How a target is probed. Serialized lowercase and rejected when unknown, so a
/// malformed configuration fails to parse rather than being repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeKind {
    Icmp,
    Tcp,
}

/// Config protocol 2 target: protocol 1 plus how to probe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPingTargetV2 {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub ip_family: i64,
    pub probe_kind: ProbeKind,
    pub port: Option<i64>,
}

impl AgentPingTargetV2 {
    /// ICMP has no port; TCP must have one in range. Checked after parsing, so a
    /// structurally valid but nonsensical target is refused rather than guessed.
    pub fn endpoint_is_valid(&self) -> bool {
        match self.probe_kind {
            ProbeKind::Icmp => self.port.is_none(),
            ProbeKind::Tcp => self.port.is_some_and(|port| (1..=65_535).contains(&port)),
        }
    }

    /// A protocol 1 target is implicitly ICMP with no port.
    pub fn from_legacy(target: AgentPingTarget) -> Self {
        Self {
            id: target.id,
            name: target.name,
            host: target.host,
            ip_family: target.ip_family,
            probe_kind: ProbeKind::Icmp,
            port: None,
        }
    }

    /// The protocol 1 projection of this target, for serving an old Agent.
    pub fn to_legacy(&self) -> AgentPingTarget {
        AgentPingTarget {
            id: self.id,
            name: self.name.clone(),
            host: self.host.clone(),
            ip_family: self.ip_family,
        }
    }
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

#[cfg(test)]
mod config_protocol_tests {
    use super::*;

    /// The exact protocol 1 target shape a v0.1.2 Agent parses. Pinned because
    /// that Agent uses deny_unknown_fields: one extra key and it rejects the whole
    /// configuration.
    #[test]
    fn config_protocol_one_target_json_is_frozen() {
        let target = AgentPingTarget {
            id: 1,
            name: "Cloudflare".to_owned(),
            host: "1.1.1.1".to_owned(),
            ip_family: 4,
        };
        assert_eq!(
            serde_json::to_string(&target).expect("serialize v1 target"),
            r#"{"id":1,"name":"Cloudflare","host":"1.1.1.1","ip_family":4}"#
        );
        let value = serde_json::to_value(&target).expect("v1 value");
        let object = value.as_object().expect("object");
        assert_eq!(object.len(), 4);
        assert!(object.get("probe_kind").is_none());
        assert!(object.get("port").is_none());
    }

    #[test]
    fn config_protocol_two_target_json_carries_the_probe_fields() {
        let tcp = AgentPingTargetV2 {
            id: 1,
            name: "HTTPS".to_owned(),
            host: "example.com".to_owned(),
            ip_family: 4,
            probe_kind: ProbeKind::Tcp,
            port: Some(443),
        };
        assert_eq!(
            serde_json::to_string(&tcp).expect("serialize tcp target"),
            r#"{"id":1,"name":"HTTPS","host":"example.com","ip_family":4,"probe_kind":"tcp","port":443}"#
        );
        let icmp = AgentPingTargetV2 {
            probe_kind: ProbeKind::Icmp,
            port: None,
            ..tcp
        };
        assert_eq!(
            serde_json::to_string(&icmp).expect("serialize icmp target"),
            r#"{"id":1,"name":"HTTPS","host":"example.com","ip_family":4,"probe_kind":"icmp","port":null}"#
        );
    }

    /// A new Agent must accept a protocol 1 body, which is what an old Server
    /// returns when it ignores the version header.
    #[test]
    fn a_protocol_one_body_parses_as_implicit_icmp() {
        let body = r#"{"protocol_version":1,"report_interval_seconds":2,"ping_interval_seconds":15,
            "targets":[{"id":7,"name":"Legacy","host":"2001:db8::1","ip_family":6}]}"#;
        let config = AgentConfigPayloadV2::from_json(body).expect("accept v1");
        assert_eq!(config.protocol_version, 1);
        assert_eq!(config.targets.len(), 1);
        assert_eq!(config.targets[0].probe_kind, ProbeKind::Icmp);
        assert_eq!(config.targets[0].port, None);
        assert_eq!(config.targets[0].host, "2001:db8::1");
        assert!(config.targets[0].endpoint_is_valid());
    }

    #[test]
    fn a_protocol_two_body_parses_both_kinds() {
        let body = r#"{"protocol_version":2,"report_interval_seconds":2,"ping_interval_seconds":15,
            "targets":[
              {"id":1,"name":"Ping","host":"1.1.1.1","ip_family":4,"probe_kind":"icmp","port":null},
              {"id":2,"name":"HTTPS","host":"example.com","ip_family":4,"probe_kind":"tcp","port":443}
            ]}"#;
        let config = AgentConfigPayloadV2::from_json(body).expect("accept v2");
        assert_eq!(config.protocol_version, 2);
        assert_eq!(config.targets[0].probe_kind, ProbeKind::Icmp);
        assert_eq!(config.targets[1].probe_kind, ProbeKind::Tcp);
        assert_eq!(config.targets[1].port, Some(443));
    }

    /// Malformed protocol 2 is refused, never repaired and never quietly
    /// reinterpreted as protocol 1.
    #[test]
    fn malformed_config_is_rejected_rather_than_repaired() {
        let target = |extra: &str| {
            format!(
                r#"{{"protocol_version":2,"report_interval_seconds":2,"ping_interval_seconds":15,
                  "targets":[{{"id":1,"name":"T","host":"example.com","ip_family":4,{extra}}}]}}"#
            )
        };
        for (label, body) in [
            (
                "tcp without a port",
                target(r#""probe_kind":"tcp","port":null"#),
            ),
            (
                "icmp with a port",
                target(r#""probe_kind":"icmp","port":443"#),
            ),
            ("port zero", target(r#""probe_kind":"tcp","port":0"#)),
            (
                "port above range",
                target(r#""probe_kind":"tcp","port":65536"#),
            ),
            ("negative port", target(r#""probe_kind":"tcp","port":-1"#)),
        ] {
            assert_eq!(
                AgentConfigPayloadV2::from_json(&body),
                Err(ConfigParseError::InvalidEndpoint),
                "{label}"
            );
        }
        for (label, body) in [
            ("unknown kind", target(r#""probe_kind":"udp","port":53"#)),
            ("uppercase kind", target(r#""probe_kind":"TCP","port":443"#)),
            ("missing kind", target(r#""port":443"#)),
            (
                "unknown target field",
                target(r#""probe_kind":"tcp","port":443,"extra":1"#),
            ),
        ] {
            assert_eq!(
                AgentConfigPayloadV2::from_json(&body),
                Err(ConfigParseError::Malformed),
                "{label}"
            );
        }
        assert_eq!(
            AgentConfigPayloadV2::from_json(
                r#"{"protocol_version":3,"report_interval_seconds":2,"ping_interval_seconds":15,"targets":[]}"#
            ),
            Err(ConfigParseError::UnsupportedVersion(3))
        );
        assert_eq!(
            AgentConfigPayloadV2::from_json("not json"),
            Err(ConfigParseError::Malformed)
        );
    }
}
