use config::{Environment, File};
use derivative::Derivative;
use serde::Deserialize;
use std::cmp::min;
use std::fmt::{Debug, Display, Formatter};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

#[derive(Deserialize, Derivative, Clone, PartialEq)]
#[derivative(Debug)]
pub struct Config {
    pub listen_address: SocketAddr,
    pub relayer_urls: Vec<Url>,
    #[serde(default)]
    pub relayer_connections: Option<usize>,
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
    pub proxy_url: Option<RedactedUrl>,
    #[serde(with = "humantime_serde", default = "default_legacy_sched_interval")]
    pub legacy_sched_interval_duration: Duration,
}

/// A `Url` that never renders embedded credentials. The proxy URL may carry
/// Basic-auth userinfo (`user:pass@host`), and `url::Url`'s own `Display`/`Debug`
/// both emit the password in plaintext. This wrapper redacts userinfo (and
/// path/query) on every `Display`/`Debug`, so it cannot leak into logs or spans.
/// Code that genuinely needs the credentials reaches them through [`RedactedUrl::expose`].
#[derive(Deserialize, Clone, PartialEq)]
#[serde(transparent)]
pub struct RedactedUrl(Url);

impl RedactedUrl {
    /// Access the underlying `Url`, including any embedded credentials. Named to
    /// keep credential exposure explicit and greppable at every call site.
    pub fn expose(&self) -> &Url {
        &self.0
    }
}

/// `scheme://host[:port]` with userinfo, path, and query stripped.
fn redact_url(url: &Url) -> String {
    let scheme = url.scheme();
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{scheme}://{host}:{port}"),
        (Some(host), None) => format!("{scheme}://{host}"),
        (None, _) => scheme.to_string(),
    }
}

impl Display for RedactedUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&redact_url(&self.0))
    }
}

impl Debug for RedactedUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", redact_url(&self.0))
    }
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
    "https://pyth.dourolabs.app/v1/symbols"
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

#[cfg(test)]
mod tests {
    use super::RedactedUrl;
    use url::Url;

    fn redacted(s: &str) -> RedactedUrl {
        RedactedUrl(Url::parse(s).unwrap())
    }

    #[test]
    fn redacted_url_hides_credentials() {
        let url = redacted("http://alice:s3cr3t@proxy.example.com:8080/path?q=1");

        let display = format!("{url}");
        let debug = format!("{url:?}");
        assert_eq!(display, "http://proxy.example.com:8080");
        assert_eq!(debug, "\"http://proxy.example.com:8080\"");
        for rendered in [&display, &debug] {
            assert!(!rendered.contains("alice"));
            assert!(!rendered.contains("s3cr3t"));
        }

        // The credentials remain reachable through the explicit accessor.
        assert_eq!(url.expose().password(), Some("s3cr3t"));

        // No explicit port still renders cleanly without userinfo.
        let no_port = redacted("https://bob:hunter2@proxy.example.com");
        assert_eq!(format!("{no_port}"), "https://proxy.example.com");
    }
}
