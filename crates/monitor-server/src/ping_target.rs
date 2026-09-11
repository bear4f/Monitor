use std::net::IpAddr;

use monitor_common::ProbeKind;

/// A target endpoint split into its canonical stored parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedEndpoint {
    pub host: String,
    pub port: Option<i64>,
}

/// Characters an endpoint may contain. Everything else -- schemes, paths,
/// userinfo, query strings and every shell metacharacter -- is refused by this
/// one rule rather than by a list of forbidden forms.
fn endpoint_charset_is_safe(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 262
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
}

fn valid_port_text(value: &str) -> Option<i64> {
    if value.len() > 5 || value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if value.len() > 1 && value.starts_with('0') {
        return None;
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|port| (1..=65_535).contains(port))
}

/// The authoritative endpoint parser. The browser does not reimplement it: an
/// admin request carries one `target` string and this decides what is stored.
///
/// ICMP takes a bare host, including a bare IPv6 literal, because there is no
/// port to make the colons ambiguous. TCP requires a port, and an IPv6 literal
/// must be bracketed so `2001:db8::1:443` is refused as ambiguous rather than
/// guessed at.
pub(crate) fn parse_endpoint(
    kind: ProbeKind,
    value: &str,
    ip_family: i64,
) -> Option<ParsedEndpoint> {
    if !endpoint_charset_is_safe(value) {
        return None;
    }
    match kind {
        ProbeKind::Icmp => {
            if value.contains('[') || value.contains(']') {
                return None;
            }
            valid_host_for_family(value, ip_family).then(|| ParsedEndpoint {
                host: value.to_owned(),
                port: None,
            })
        }
        ProbeKind::Tcp => {
            let (host, port) = if let Some(rest) = value.strip_prefix('[') {
                let (host, port) = rest.split_once("]:")?;
                // Brackets exist to disambiguate an IPv6 literal; they may not wrap
                // a hostname or an IPv4 literal.
                if !matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
                    return None;
                }
                (host, port)
            } else {
                if value.bytes().filter(|byte| *byte == b':').count() != 1 {
                    return None;
                }
                value.split_once(':')?
            };
            if host.contains('[') || host.contains(']') {
                return None;
            }
            let port = valid_port_text(port)?;
            valid_host_for_family(host, ip_family).then(|| ParsedEndpoint {
                host: host.to_owned(),
                port: Some(port),
            })
        }
    }
}

/// Whether a stored triple is a coherent endpoint. Used to validate the *final*
/// state of a patch, so changing only the kind cannot leave an ICMP target with
/// a port or a TCP target without one.
pub(crate) fn endpoint_is_valid(
    kind: ProbeKind,
    host: &str,
    port: Option<i64>,
    family: i64,
) -> bool {
    if !valid_host_for_family(host, family) {
        return false;
    }
    match kind {
        ProbeKind::Icmp => port.is_none(),
        ProbeKind::Tcp => port.is_some_and(|port| (1..=65_535).contains(&port)),
    }
}

pub(crate) fn probe_kind_from_str(value: &str) -> Option<ProbeKind> {
    match value {
        "icmp" => Some(ProbeKind::Icmp),
        "tcp" => Some(ProbeKind::Tcp),
        _ => None,
    }
}

pub(crate) fn probe_kind_as_str(kind: ProbeKind) -> &'static str {
    match kind {
        ProbeKind::Icmp => "icmp",
        ProbeKind::Tcp => "tcp",
    }
}

pub(crate) fn valid_host_for_family(host: &str, ip_family: i64) -> bool {
    if host.is_empty()
        || host.len() > 253
        || !host.is_ascii()
        || host.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return false;
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        return matches!(
            (address, ip_family),
            (IpAddr::V4(_), 4) | (IpAddr::V6(_), 6)
        );
    }
    if !matches!(ip_family, 4 | 6) {
        return false;
    }
    host.split('.').all(|label| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_literals_and_strict_ascii_hostnames() {
        for (host, family) in [
            ("203.0.113.1", 4),
            ("2001:db8::1", 6),
            ("example.com", 4),
            ("EXAMPLE.com", 6),
            ("xn--bcher-kva.example", 4),
        ] {
            assert!(valid_host_for_family(host, family), "{host}");
        }
        for (host, family) in [
            ("203.0.113.1", 6),
            ("2001:db8::1", 4),
            ("https://example.com", 4),
            ("example.com:443", 4),
            ("example.com/path", 4),
            ("user@example.com", 4),
            ("example.com?q=x", 4),
            ("example.com ", 4),
            ("bad_name.example", 4),
            ("abc..def", 4),
            ("-abc.example", 4),
            ("abc-.example", 4),
            ("$(id)", 4),
            (";reboot", 4),
        ] {
            assert!(!valid_host_for_family(host, family), "{host}");
        }
        assert!(!valid_host_for_family(
            &format!("{}.example", "a".repeat(64)),
            4
        ));
        assert!(!valid_host_for_family(&"a".repeat(254), 4));
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn icmp_accepts_bare_hosts_including_bare_ipv6() {
        for (value, family) in [
            ("example.com", 4),
            ("example.com", 6),
            ("1.1.1.1", 4),
            ("8.8.8.8", 4),
            ("2001:db8::1", 6),
            ("::1", 6),
        ] {
            let parsed = parse_endpoint(ProbeKind::Icmp, value, family)
                .unwrap_or_else(|| panic!("icmp {value} family {family} must be accepted"));
            assert_eq!(parsed.host, value);
            assert_eq!(parsed.port, None, "icmp must never carry a port");
        }
    }

    #[test]
    fn icmp_rejects_ports_urls_and_shell_shapes() {
        for (value, family) in [
            ("https://example.com", 4),
            ("http://example.com", 4),
            ("example.com/path", 4),
            ("example.com ", 4),
            (" example.com", 4),
            ("exa mple.com", 4),
            ("host:80", 4),
            ("example.com:80", 4),
            ("[2001:db8::1]:80", 6),
            ("[2001:db8::1]", 6),
            ("user@example.com", 4),
            ("example.com?q=1", 4),
            ("$(id)", 4),
            (";reboot", 4),
            ("a|b", 4),
            ("a&b", 4),
            ("", 4),
            ("203.0.113.1", 6),
            ("2001:db8::1", 4),
        ] {
            assert!(
                parse_endpoint(ProbeKind::Icmp, value, family).is_none(),
                "icmp {value:?} family {family} must be rejected"
            );
        }
    }

    #[test]
    fn tcp_accepts_host_port_and_bracketed_ipv6() {
        for (value, family, host, port) in [
            ("example.com:80", 4, "example.com", 80),
            ("example.com:443", 6, "example.com", 443),
            ("1.1.1.1:443", 4, "1.1.1.1", 443),
            ("59.51.71.209:443", 4, "59.51.71.209", 443),
            ("[2001:db8::1]:443", 6, "2001:db8::1", 443),
            ("[::1]:1", 6, "::1", 1),
            ("example.com:65535", 4, "example.com", 65_535),
        ] {
            let parsed = parse_endpoint(ProbeKind::Tcp, value, family)
                .unwrap_or_else(|| panic!("tcp {value} must be accepted"));
            assert_eq!(parsed.host, host);
            assert_eq!(parsed.port, Some(port));
        }
    }

    #[test]
    fn tcp_rejects_missing_ports_ambiguous_ipv6_and_bad_ports() {
        for (value, family) in [
            ("example.com", 4),
            ("1.1.1.1", 4),
            ("2001:db8::1", 6),
            // Ambiguous: is the last group a port or part of the address?
            ("2001:db8::1:443", 6),
            ("::1:443", 6),
            ("[2001:db8::1]", 6),
            ("[2001:db8::1]:", 6),
            ("example.com:", 4),
            (":443", 4),
            ("example.com:0", 4),
            ("example.com:65536", 4),
            ("example.com:-1", 4),
            ("example.com:+80", 4),
            ("example.com:080", 4),
            ("example.com:8o", 4),
            ("example.com:123456", 4),
            ("http://example.com:80", 4),
            ("user@example.com:80", 4),
            ("example.com:80/path", 4),
            ("example.com:80 ", 4),
            ("[example.com]:80", 4),
            ("1.1.1.1:443", 6),
            ("[2001:db8::1]:443", 4),
        ] {
            assert!(
                parse_endpoint(ProbeKind::Tcp, value, family).is_none(),
                "tcp {value:?} family {family} must be rejected"
            );
        }
    }

    /// What a patch is judged by: the resulting triple, not the supplied fields.
    #[test]
    fn final_state_validation_pairs_kind_with_port() {
        assert!(endpoint_is_valid(ProbeKind::Icmp, "example.com", None, 4));
        assert!(endpoint_is_valid(
            ProbeKind::Tcp,
            "example.com",
            Some(443),
            4
        ));
        assert!(endpoint_is_valid(ProbeKind::Tcp, "2001:db8::1", Some(1), 6));
        // Turning an ICMP target into TCP without giving it a port.
        assert!(!endpoint_is_valid(ProbeKind::Tcp, "example.com", None, 4));
        // Turning a TCP target into ICMP while it still has a port.
        assert!(!endpoint_is_valid(
            ProbeKind::Icmp,
            "example.com",
            Some(443),
            4
        ));
        assert!(!endpoint_is_valid(
            ProbeKind::Tcp,
            "example.com",
            Some(0),
            4
        ));
        assert!(!endpoint_is_valid(
            ProbeKind::Tcp,
            "example.com",
            Some(65_536),
            4
        ));
        assert!(!endpoint_is_valid(ProbeKind::Icmp, "203.0.113.1", None, 6));
    }

    #[test]
    fn probe_kind_text_round_trips_and_refuses_anything_else() {
        assert_eq!(probe_kind_from_str("icmp"), Some(ProbeKind::Icmp));
        assert_eq!(probe_kind_from_str("tcp"), Some(ProbeKind::Tcp));
        for invalid in ["", "ICMP", "Tcp", "udp", "http", "icmp ", " tcp"] {
            assert_eq!(probe_kind_from_str(invalid), None, "{invalid:?}");
        }
        assert_eq!(probe_kind_as_str(ProbeKind::Icmp), "icmp");
        assert_eq!(probe_kind_as_str(ProbeKind::Tcp), "tcp");
    }
}
