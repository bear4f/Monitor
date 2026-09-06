use monitor_server::{config::Config, run};

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
        Err(error) => {
            eprintln!("configuration error: {error}");
            std::process::exit(2);
        }
    };

    if let Err(error) = run(config).await {
        eprintln!("startup error: {error}");
        std::process::exit(1);
    }
}
