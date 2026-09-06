const SERVICE: &str = include_str!("../../../packaging/monitor-agent.service");
const ENVIRONMENT_EXAMPLE: &str = include_str!("../../../packaging/monitor-agent.env.example");

fn directive(name: &str) -> Vec<&str> {
    SERVICE
        .lines()
        .filter_map(|line| line.strip_prefix(name))
        .collect()
}

#[test]
fn systemd_exec_uses_root_owned_environment_file_without_token_in_argv() {
    assert_eq!(
        directive("EnvironmentFile="),
        vec!["/etc/monitor-agent.env"]
    );
    assert_eq!(
        directive("ExecStart="),
        vec!["/usr/local/bin/monitor-agent"]
    );
    let exec_start = directive("ExecStart=").join(" ");
    assert!(!exec_start.contains("--token"));
    assert!(!exec_start.contains("MONITOR_TOKEN"));
    assert!(ENVIRONMENT_EXAMPLE.contains("owner root:root and mode 0600"));
    assert!(!ENVIRONMENT_EXAMPLE.contains("0123456789abcdef"));
}

#[test]
fn systemd_service_uses_dedicated_identity_and_only_net_raw_capability() {
    assert_eq!(directive("User="), vec!["monitor-agent"]);
    assert_eq!(directive("Group="), vec!["monitor-agent"]);
    assert!(SERVICE.contains("system account with no home and a nologin shell"));
    assert_eq!(directive("CapabilityBoundingSet="), vec!["CAP_NET_RAW"]);
    assert_eq!(directive("AmbientCapabilities="), vec!["CAP_NET_RAW"]);
    for forbidden in [
        "CAP_NET_ADMIN",
        "CAP_SYS_ADMIN",
        "CAP_DAC_OVERRIDE",
        "CAP_SYS_PTRACE",
        "CAP_SYS_RESOURCE",
        "CAP_SETUID",
        "CAP_SETGID",
    ] {
        assert!(!SERVICE.contains(forbidden));
    }
}

#[test]
fn systemd_service_contains_required_hardening_without_hiding_proc() {
    for required in [
        "NoNewPrivileges=true",
        "PrivateTmp=true",
        "ProtectSystem=strict",
        "ProtectHome=true",
        "ProtectKernelTunables=true",
        "ProtectKernelModules=true",
        "ProtectControlGroups=true",
        "ReadOnlyPaths=/proc /sys /etc",
        "Restart=on-failure",
        "RestartSec=5s",
    ] {
        assert!(SERVICE.lines().any(|line| line == required), "{required}");
    }
    assert!(!SERVICE.contains("ProtectProc=invisible"));
    assert!(!SERVICE.contains("PrivateUsers="));
    assert!(!SERVICE.contains("SystemCallFilter="));
}
