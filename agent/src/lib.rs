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
    let (identifier_map, refresh) =
        metadata::spawn_identifier_map_refresh(config.history_service_url.clone());
    tokio::select! {
        result = http_server::run(config, lazer_publisher, identifier_map) => result,
        result = refresh => anyhow::bail!("identifier map refresh task ended: {result:?}"),
    }
}
