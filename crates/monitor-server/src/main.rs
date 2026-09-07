use monitor_server::{
    admin_cli,
    config::{Config, ServerCommand, VERSION},
    run,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .init();

    let config = match Config::parse_env() {
        Ok(config) => config,
        Err(monitor_server::config::ConfigError::HelpRequested) => {
            println!("{}", monitor_server::config::ConfigError::usage());
            return;
        }
        Err(monitor_server::config::ConfigError::VersionRequested) => {
            println!("monitor-server {VERSION}");
            return;
        }
        Err(error) => {
            eprintln!("configuration error: {error}");
            std::process::exit(2);
        }
    };

    match config.command {
        ServerCommand::Serve => {
            if let Err(error) = run(config).await {
                eprintln!("server error: {error}");
                std::process::exit(1);
            }
        }
        ServerCommand::SetAdminPassword => {
            if let Err(error) = admin_cli::set_password_from_stdio(&config.database_path).await {
                eprintln!("admin command failed: {error}");
                std::process::exit(1);
            }
        }
    }
}
