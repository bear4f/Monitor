pub mod cli;
pub mod client;
pub mod collector;
pub mod ping;
pub mod runtime;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
