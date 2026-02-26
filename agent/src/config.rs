use config::{Environment, File};
use derivative::Derivative;
use serde::Deserialize;
use std::cmp::min;
use std::fmt::{Debug, Formatter};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

#[derive(Deserialize, Derivative, Clone, PartialEq)]
#[derivative(Debug)]
pub struct Config {
    pub listen_address: SocketAddr,
    pub relayer_urls: Vec<Url>,
    pub authorization_token: Option<AuthorizationToken>,
    #[derivative(Debug = "ignore")]
    pub publish_keypair_path: PathBuf,
    #[serde(with = "humantime_serde", default = "default_publish_interval")]
    pub publish_interval_duration: Duration,
    #[serde(default = "default_history_service_url")]
    pub history_service_url: Url,
    #[serde(default)]
    pub enable_update_deduplication: bool,
    #[serde(with = "humantime_serde", default = "default_update_deduplication_ttl")]
    pub update_deduplication_ttl: Duration,
    pub proxy_url: Option<Url>,
    #[serde(with = "humantime_serde", default = "default_legacy_sched_interval")]
    pub legacy_sched_interval_duration: Duration,
}

#[derive(Deserialize, Derivative, Clone, PartialEq)]
pub struct AuthorizationToken(pub String);

impl Debug for AuthorizationToken {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let token_string = self.0.to_ascii_lowercase();
        #[allow(clippy::string_slice, reason = "false positive")]
        let last_chars = &token_string[token_string.len() - min(4, token_string.len())..];
        write!(f, "\"...{last_chars}\"")
    }
}

fn default_publish_interval() -> Duration {
    Duration::from_millis(25)
}

fn default_update_deduplication_ttl() -> Duration {
    Duration::from_millis(500)
}

fn default_legacy_sched_interval() -> Duration {
    Duration::from_millis(500)
}

fn default_history_service_url() -> Url {
    #[allow(clippy::expect_used, reason = "hardcoded URL is always valid")]
    "https://history.pyth-lazer.dourolabs.app/history/v1/symbols"
        .parse()
        .expect("hardcoded URL is valid")
}

pub fn load_config(config_path: String) -> anyhow::Result<Config> {
    let config = config::Config::builder()
        .add_source(File::with_name(&config_path))
        .add_source(Environment::with_prefix("LAZER_AGENT").separator("__"))
        .build()?
        .try_deserialize()?;
    Ok(config)
}

// Default capacity for all tokio mpsc channels that communicate between tasks.
pub const CHANNEL_CAPACITY: usize = 1000;
