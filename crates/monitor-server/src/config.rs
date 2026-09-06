use std::{
    ffi::OsString,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

const DEFAULT_LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25_774);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub command: ServerCommand,
    pub listen: SocketAddr,
    pub database_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerCommand {
    Serve,
    SetAdminPassword,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command: ServerCommand::Serve,
            listen: DEFAULT_LISTEN,
            database_path: PathBuf::from("monitor.db"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    HelpRequested,
    MissingValue(&'static str),
    InvalidListen(String),
    EmptyDatabasePath,
    UnknownArgument(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HelpRequested => formatter.write_str(Self::usage()),
            Self::MissingValue(flag) => write!(formatter, "missing value for {flag}"),
            Self::InvalidListen(value) => write!(formatter, "invalid --listen address: {value}"),
            Self::EmptyDatabasePath => formatter.write_str("--db path must not be empty"),
            Self::UnknownArgument(value) => write!(formatter, "unknown argument: {value}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl ConfigError {
    pub const fn usage() -> &'static str {
        "Usage:\n  monitor-server [--listen IP:PORT] [--db PATH]\n  monitor-server admin set-password [--db PATH]\n\nThe password is read from one line on stdin; piping stdin is recommended."
    }
}

impl Config {
    pub fn parse_env() -> Result<Self, ConfigError> {
        Self::parse(std::env::args_os().skip(1))
    }

    pub fn parse<I>(arguments: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut config = Self::default();
        let mut arguments = arguments.into_iter();
        let mut listen_was_set = false;

        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--listen") if config.command == ServerCommand::Serve => {
                    let value = arguments
                        .next()
                        .ok_or(ConfigError::MissingValue("--listen"))?;
                    let value = value.to_str().ok_or_else(|| {
                        ConfigError::InvalidListen(value.to_string_lossy().into())
                    })?;
                    config.listen = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidListen(value.into()))?;
                    listen_was_set = true;
                }
                Some("--db") => {
                    let value = arguments.next().ok_or(ConfigError::MissingValue("--db"))?;
                    if value.is_empty() {
                        return Err(ConfigError::EmptyDatabasePath);
                    }
                    config.database_path = PathBuf::from(value);
                }
                Some("admin") if config.command == ServerCommand::Serve && !listen_was_set => {
                    match arguments.next().as_deref().and_then(|value| value.to_str()) {
                        Some("set-password") => config.command = ServerCommand::SetAdminPassword,
                        Some(value) => {
                            return Err(ConfigError::UnknownArgument(format!("admin {value}")));
                        }
                        None => return Err(ConfigError::MissingValue("admin")),
                    }
                }
                Some("--help" | "-h") => return Err(ConfigError::HelpRequested),
                _ => {
                    return Err(ConfigError::UnknownArgument(
                        argument.to_string_lossy().into(),
                    ));
                }
            }
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_arguments() {
        let config = Config::parse([
            OsString::from("--listen"),
            OsString::from("0.0.0.0:3000"),
            OsString::from("--db"),
            OsString::from("data/test.db"),
        ])
        .expect("valid arguments");

        assert_eq!(config.listen, "0.0.0.0:3000".parse().unwrap());
        assert_eq!(config.database_path, PathBuf::from("data/test.db"));
        assert_eq!(config.command, ServerCommand::Serve);
    }

    #[test]
    fn rejects_unknown_arguments() {
        let error = Config::parse([OsString::from("--health")]).unwrap_err();
        assert_eq!(error, ConfigError::UnknownArgument("--health".into()));
    }

    #[test]
    fn parses_admin_set_password_command_without_password_argument() {
        let config = Config::parse([
            OsString::from("admin"),
            OsString::from("set-password"),
            OsString::from("--db"),
            OsString::from("private.db"),
        ])
        .expect("valid admin command");

        assert_eq!(config.command, ServerCommand::SetAdminPassword);
        assert_eq!(config.database_path, PathBuf::from("private.db"));

        let documented_order = Config::parse([
            OsString::from("--db"),
            OsString::from("private.db"),
            OsString::from("admin"),
            OsString::from("set-password"),
        ])
        .expect("valid documented admin command order");
        assert_eq!(documented_order, config);
    }

    #[test]
    fn rejects_password_on_command_line() {
        let error = Config::parse([
            OsString::from("admin"),
            OsString::from("set-password"),
            OsString::from("--password"),
            OsString::from("hunter2"),
        ])
        .unwrap_err();

        assert_eq!(error, ConfigError::UnknownArgument("--password".into()));
    }
}
