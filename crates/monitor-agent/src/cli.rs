use std::{error::Error, fmt};

#[derive(Clone)]
pub struct SecretToken(String);

impl SecretToken {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretToken([redacted])")
    }
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub server: String,
    pub token: SecretToken,
}

#[derive(Debug, Clone)]
pub enum Action {
    Run(RunConfig),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CliError {}

pub fn usage() -> &'static str {
    "Usage: monitor-agent --server <http(s)://host[:port]> --token <64-lowercase-hex>\n\
     Environment fallback: MONITOR_SERVER, MONITOR_TOKEN\n\
     Options:\n  --help       Show this help\n  --version    Show version"
}

pub fn parse<I>(args: I) -> Result<Action, CliError>
where
    I: IntoIterator<Item = String>,
{
    parse_with_env(args, |name| std::env::var(name).ok())
}

fn parse_with_env<I, F>(args: I, environment: F) -> Result<Action, CliError>
where
    I: IntoIterator<Item = String>,
    F: Fn(&str) -> Option<String>,
{
    let mut args = args.into_iter();
    let _program = args.next();
    let mut server = None;
    let mut token = None;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" => {
                if server.is_some() || token.is_some() || args.next().is_some() {
                    return Err(CliError(
                        "--help cannot be combined with arguments".to_owned(),
                    ));
                }
                return Ok(Action::Help);
            }
            "--version" => {
                if server.is_some() || token.is_some() || args.next().is_some() {
                    return Err(CliError(
                        "--version cannot be combined with arguments".to_owned(),
                    ));
                }
                return Ok(Action::Version);
            }
            "--server" => {
                if server.is_some() {
                    return Err(CliError("duplicate --server".to_owned()));
                }
                server = Some(
                    args.next()
                        .ok_or_else(|| CliError("missing value for --server".to_owned()))?,
                );
            }
            "--token" => {
                if token.is_some() {
                    return Err(CliError("duplicate --token".to_owned()));
                }
                token = Some(
                    args.next()
                        .ok_or_else(|| CliError("missing value for --token".to_owned()))?,
                );
            }
            _ => return Err(CliError(format!("unknown argument: {argument}"))),
        }
    }

    let server = server.or_else(|| environment("MONITOR_SERVER"));
    let token = token.or_else(|| environment("MONITOR_TOKEN"));
    let server = normalize_server_url(
        &server
            .ok_or_else(|| CliError("missing required --server or MONITOR_SERVER".to_owned()))?,
    )?;
    let token =
        token.ok_or_else(|| CliError("missing required --token or MONITOR_TOKEN".to_owned()))?;
    if !valid_token(&token) {
        return Err(CliError(
            "--token must be exactly 64 lowercase hexadecimal characters".to_owned(),
        ));
    }
    Ok(Action::Run(RunConfig {
        server,
        token: SecretToken(token),
    }))
}

fn valid_token(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn normalize_server_url(value: &str) -> Result<String, CliError> {
    if value.is_empty()
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
        || value.bytes().any(|byte| matches!(byte, b'?' | b'#' | b'@'))
    {
        return Err(CliError("invalid --server URL".to_owned()));
    }
    let authority = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .ok_or_else(|| CliError("--server must use http:// or https://".to_owned()))?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.contains('/') || !valid_authority(authority) {
        return Err(CliError("invalid --server URL".to_owned()));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

fn valid_authority(authority: &str) -> bool {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, port)) = rest.split_once(']') else {
            return false;
        };
        return host.parse::<std::net::Ipv6Addr>().is_ok()
            && (port.is_empty() || valid_port(port.strip_prefix(':')));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, Some(port)),
        _ => (authority, None),
    };
    !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        && (port.is_none() || valid_port(port))
}

fn valid_port(port: Option<&str>) -> bool {
    port.and_then(|value| value.parse::<u16>().ok())
        .is_some_and(|value| value != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn arguments(values: &[&str]) -> Vec<String> {
        std::iter::once("monitor-agent")
            .chain(values.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    fn parse_without_environment(values: &[&str]) -> Result<Action, CliError> {
        parse_with_env(arguments(values), |_| None)
    }

    #[test]
    fn parses_and_normalizes_required_arguments() {
        let Action::Run(config) =
            parse_without_environment(&["--token", TOKEN, "--server", "https://[::1]:8443/"])
                .expect("valid CLI")
        else {
            panic!("expected run action");
        };
        assert_eq!(config.server, "https://[::1]:8443");
        assert_eq!(config.token.expose(), TOKEN);
        assert_eq!(format!("{:?}", config.token), "SecretToken([redacted])");
    }

    #[test]
    fn accepts_help_and_version() {
        assert!(matches!(
            parse_without_environment(&["--help"]),
            Ok(Action::Help)
        ));
        assert!(matches!(
            parse_without_environment(&["--version"]),
            Ok(Action::Version)
        ));
    }

    #[test]
    fn rejects_missing_duplicate_unknown_and_invalid_tokens() {
        for values in [
            vec!["--server", "https://example.test"],
            vec![
                "--server",
                "https://a",
                "--server",
                "https://b",
                "--token",
                TOKEN,
            ],
            vec!["--server", "https://example.test", "--token", "ABC"],
            vec!["--wat"],
        ] {
            assert!(
                parse_without_environment(&values).is_err(),
                "accepted {values:?}"
            );
        }
    }

    #[test]
    fn validates_the_minimal_root_server_url_contract() {
        for url in [
            "ftp://example.com",
            "https://user@example.com",
            "https://example.com/path",
            "https://example.com?q=1",
            "https://example.com#x",
            "https://example .com",
            "http://example.com:0",
            "http://::1",
        ] {
            assert!(normalize_server_url(url).is_err(), "accepted {url}");
        }
        for url in [
            "http://127.0.0.1:25774",
            "https://monitor.example.com/",
            "https://[2001:db8::1]:443",
        ] {
            assert!(normalize_server_url(url).is_ok(), "rejected {url}");
        }
    }

    #[test]
    fn environment_supplies_missing_server_and_token() {
        let Action::Run(config) = parse_with_env(arguments(&[]), |name| match name {
            "MONITOR_SERVER" => Some("https://monitor.example.test/".to_owned()),
            "MONITOR_TOKEN" => Some(TOKEN.to_owned()),
            _ => None,
        })
        .expect("valid environment fallback") else {
            panic!("expected run action");
        };
        assert_eq!(config.server, "https://monitor.example.test");
        assert_eq!(config.token.expose(), TOKEN);
        assert_eq!(format!("{:?}", config.token), "SecretToken([redacted])");
    }

    #[test]
    fn explicit_cli_values_override_environment() {
        let cli_token = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let Action::Run(config) = parse_with_env(
            arguments(&["--server", "https://cli.example.test", "--token", cli_token]),
            |name| match name {
                "MONITOR_SERVER" => Some("https://environment.example.test".to_owned()),
                "MONITOR_TOKEN" => Some(TOKEN.to_owned()),
                _ => None,
            },
        )
        .expect("CLI overrides environment") else {
            panic!("expected run action");
        };
        assert_eq!(config.server, "https://cli.example.test");
        assert_eq!(config.token.expose(), cli_token);
    }

    #[test]
    fn invalid_environment_token_is_rejected_without_echoing_it() {
        let invalid = "NOT-A-VALID-TOKEN";
        let error = parse_with_env(arguments(&[]), |name| match name {
            "MONITOR_SERVER" => Some("https://monitor.example.test".to_owned()),
            "MONITOR_TOKEN" => Some(invalid.to_owned()),
            _ => None,
        })
        .expect_err("invalid environment token");
        assert!(!error.to_string().contains(invalid));
    }
}
