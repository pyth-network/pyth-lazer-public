use {
    crate::resilient_http_client::{BackoffErrorForStatusExt, ResilientHttpClient},
    futures::Stream,
    pyth_lazer_protocol::jrpc::SymbolMetadata,
    serde::{Deserialize, Serialize},
    std::{path::PathBuf, sync::Arc, time::Duration},
    tracing::{Instrument, info_span},
    url::Url,
};

/// Configuration for the api client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PythLazerApiClientConfig {
    /// URLs of the api services.
    #[serde(default = "default_urls")]
    pub urls: Vec<Url>,
    /// Interval of queries to the api services.
    /// Note: if the request fails, it will be retried using exponential backoff regardless of this setting.
    #[serde(with = "humantime_serde", default = "default_update_interval")]
    pub update_interval: Duration,
    /// Timeout of an individual request.
    #[serde(with = "humantime_serde", default = "default_request_timeout")]
    pub request_timeout: Duration,
    /// Path to the cache directory that can be used to provide latest data if the api service is unavailable.
    pub cache_dir: Option<PathBuf>,
    /// Capacity of communication channels created by this client. It must be above zero.
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
}

fn default_urls() -> Vec<Url> {
    vec![Url::parse("https://pyth.dourolabs.app/").unwrap()]
}

fn default_update_interval() -> Duration {
    Duration::from_secs(30)
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(15)
}

fn default_channel_capacity() -> usize {
    1000
}

impl Default for PythLazerApiClientConfig {
    fn default() -> Self {
        Self {
            urls: default_urls(),
            update_interval: default_update_interval(),
            request_timeout: default_request_timeout(),
            cache_dir: None,
            channel_capacity: default_channel_capacity(),
        }
    }
}

/// Client to the api service.
#[derive(Debug, Clone)]
pub struct PythLazerApiClient {
    http: Arc<ResilientHttpClient>,
}

impl PythLazerApiClient {
    pub fn new(config: PythLazerApiClientConfig) -> Self {
        let reqwest = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .expect("failed to initialize reqwest");
        Self {
            http: Arc::new(ResilientHttpClient {
                urls: config.urls,
                update_interval: config.update_interval,
                channel_capacity: config.channel_capacity,
                cache_dir: config.cache_dir,
                reqwest,
            }),
        }
    }

    /// Fetch current metadata for all symbols.
    pub async fn all_symbols_metadata(&self) -> anyhow::Result<Vec<SymbolMetadata>> {
        self.http
            .fetch_or_cache(
                self.http.cache_file_path("symbols_v1.json"),
                request_symbols,
            )
            .instrument(info_span!("all_symbols_metadata"))
            .await
    }

    /// Creates a fault-tolerant stream that requests the list of symbols and yields new items
    /// when a change of value occurs.
    ///
    /// Returns an error if the initial fetch failed.
    /// On a successful return, the channel will always contain the initial data that can be fetched
    /// immediately from the returned stream.
    /// You should continuously poll the stream to receive updates.
    pub async fn all_symbols_metadata_stream(
        &self,
    ) -> anyhow::Result<impl Stream<Item = Vec<SymbolMetadata>> + use<> + Unpin> {
        self.http
            .stream(self.http.cache_file_path("symbols_v1.json"), |http, url| {
                Box::pin(request_symbols(http, url))
            })
            .instrument(info_span!("all_symbols_metadata_stream"))
            .await
    }
}

async fn request_symbols(
    http: &ResilientHttpClient,
    url: &Url,
) -> Result<Vec<SymbolMetadata>, backoff::Error<anyhow::Error>> {
    let url = url
        .join("v1/symbols")
        .map_err(|err| backoff::Error::permanent(anyhow::Error::from(err)))?;

    let response = http
        .reqwest
        .get(url.clone())
        .send()
        .await
        .map_err(|err| backoff::Error::transient(anyhow::Error::from(err)))?
        .backoff_error_for_status()?;
    let vec = response
        .json::<Vec<SymbolMetadata>>()
        .await
        .map_err(|err| backoff::Error::transient(anyhow::Error::from(err)))?;
    Ok(vec)
}
