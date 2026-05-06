use crate::lazer_publisher::LazerPublisher;

pub use crate::config::{Config, load_config};

mod config;
mod http_server;
mod jrpc_handle;
mod lazer_publisher;
mod legacy_handle;
mod metadata;
mod publisher_handle;
mod relayer_session;
mod websocket_utils;

pub async fn run(config: Config) -> anyhow::Result<()> {
    let lazer_publisher = LazerPublisher::new(&config).await;
    http_server::run(config, lazer_publisher).await
}
