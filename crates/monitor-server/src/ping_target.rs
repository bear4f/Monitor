use std::net::IpAddr;

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
