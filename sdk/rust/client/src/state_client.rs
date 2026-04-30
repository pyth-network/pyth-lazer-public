use {
    crate::resilient_http_client::{BackoffErrorForStatusExt, ResilientHttpClient},
    anyhow::format_err,
    derivative::Derivative,
    futures::{Stream, StreamExt},
    pyth_lazer_protocol::parse_proto_json,
    pyth_lazer_publisher_sdk::state::State,
    serde::{Deserialize, Serialize, de::Error as _, ser::Error as _},
    std::{path::PathBuf, sync::Arc, time::Duration},
    tracing::{Instrument, info_span},
    url::Url,
};

/// Configuration for the state client.
#[derive(Derivative, Clone, PartialEq, Serialize, Deserialize)]
#[derivative(Debug)]
pub struct PythLazerStateClientConfig {
    /// URLs of the state service backends.
    #[serde(default = "default_urls")]
    pub urls: Vec<Url>,
    /// Interval of queries to the state service.
    /// Note: if the request fails, it will be retried using exponential backoff regardless of this setting.
    #[serde(with = "humantime_serde", default = "default_update_interval")]
    pub update_interval: Duration,
    /// Timeout of an individual request.
    #[serde(with = "humantime_serde", default = "default_request_timeout")]
    pub request_timeout: Duration,
    /// Path to the cache directory that can be used to provide latest data if the state service is unavailable.
    pub cache_dir: Option<PathBuf>,
    /// Capacity of communication channels created by this client. It must be above zero.
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
    /// Access token for publisher or governance restricted endpoints.
    ///
    /// Not needed for consumer facing endpoints.
    #[derivative(Debug = "ignore")]
    pub access_token: Option<String>,
}

fn default_urls() -> Vec<Url> {
    vec![Url::parse("https://history.pyth-lazer.dourolabs.app/").unwrap()]
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

impl Default for PythLazerStateClientConfig {
    fn default() -> Self {
        Self {
            urls: default_urls(),
            update_interval: default_update_interval(),
            request_timeout: default_request_timeout(),
            cache_dir: None,
            channel_capacity: default_channel_capacity(),
            access_token: None,
        }
    }
}

/// Client for fetching Pyth Lazer state snapshots.
#[derive(Debug, Clone)]
pub struct PythLazerStateClient {
    http: Arc<ResilientHttpClient>,
    access_token: Option<String>,
}

/// Specifies which parts of the state should be present in the output.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GetStateParams {
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub publishers: bool,
    #[serde(default)]
    pub feeds: bool,
    #[serde(default)]
    pub governance_sources: bool,
    #[serde(default)]
    pub feature_flags: bool,
    #[serde(default)]
    pub exchanges: bool,
}

impl PythLazerStateClient {
    pub fn new(config: PythLazerStateClientConfig) -> Self {
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
            access_token: config.access_token,
        }
    }

    fn state_cache_file_path(&self, params: &GetStateParams) -> Option<PathBuf> {
        let GetStateParams {
            all,
            publishers,
            feeds,
            governance_sources,
            feature_flags,
            exchanges,
        } = params;

        self.http.cache_file_path(&format!(
            "state_{}{}{}{}{}{}_v1.json",
            *all as u8,
            *publishers as u8,
            *feeds as u8,
            *governance_sources as u8,
            *feature_flags as u8,
            *exchanges as u8,
        ))
    }

    /// Fetch a partial state snapshot containing data specified in `params`.
    pub async fn state(&self, params: GetStateParams) -> anyhow::Result<State> {
        let cache_path = self.state_cache_file_path(&params);
        let access_token = self.access_token.clone();
        self.http
            .fetch_or_cache(cache_path, move |http, url| {
                let params = params.clone();
                let access_token = access_token.clone();
                async move { request_state(http, url, params, access_token).await }
            })
            .instrument(info_span!("state"))
            .await
            .map(|s| s.0)
    }

    /// Fetch a part of the current state specified by `params`.
    ///
    /// Creates a fault-tolerant stream that requests a partial state snapshot
    /// containing data specified in `params`. It yields new items
    /// when a change of value occurs.
    ///
    /// Returns an error if the initial fetch failed.
    /// On a successful return, the stream will always contain the initial data that can be fetched
    /// immediately from the returned stream.
    /// You should continuously poll the stream to receive updates.
    pub async fn state_stream(
        &self,
        params: GetStateParams,
    ) -> anyhow::Result<impl Stream<Item = State> + use<> + Unpin> {
        let cache_path = self.state_cache_file_path(&params);
        let access_token = self.access_token.clone();
        let stream = self
            .http
            .stream(cache_path, move |http, url| {
                let params = params.clone();
                let access_token = access_token.clone();
                Box::pin(async move { request_state(http, url, params, access_token).await })
            })
            .instrument(info_span!("state_stream"))
            .await?;
        Ok(stream.map(|s| s.0))
    }
}

/// Fetch data from /v1/state endpoint without any timeouts or retries.
async fn request_state(
    http: &ResilientHttpClient,
    url: &Url,
    params: GetStateParams,
    access_token: Option<String>,
) -> Result<StateWithSerde, backoff::Error<anyhow::Error>> {
    let url = url
        .join("v1/state")
        .map_err(|err| backoff::Error::permanent(anyhow::Error::from(err)))?;
    let access_token = access_token
        .ok_or_else(|| backoff::Error::permanent(format_err!("missing access_token in config")))?;
    let response = http
        .reqwest
        .get(url.clone())
        .query(&params)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|err| {
            backoff::Error::transient(
                anyhow::Error::from(err).context(format!("failed to fetch state from {url}")),
            )
        })?
        .backoff_error_for_status()?;
    let bytes = response.bytes().await.map_err(|err| {
        backoff::Error::transient(
            anyhow::Error::from(err).context(format!("failed to fetch state from {url}")),
        )
    })?;
    let json = String::from_utf8(bytes.into()).map_err(|err| {
        backoff::Error::permanent(
            anyhow::Error::from(err).context(format!("failed to parse state from {url}")),
        )
    })?;
    let state = parse_proto_json::<State>(&json).map_err(|err| {
        backoff::Error::permanent(
            anyhow::Error::from(err).context(format!("failed to parse state from {url}")),
        )
    })?;
    Ok(StateWithSerde(state))
}

// State wrapper that delegates serialization and deserialization to `protobuf_json_mapping`.
#[derive(Debug, Clone, PartialEq)]
struct StateWithSerde(State);

impl Serialize for StateWithSerde {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let json = protobuf_json_mapping::print_to_string(&self.0).map_err(S::Error::custom)?;
        let json_value =
            serde_json::from_str::<serde_json::Value>(&json).map_err(S::Error::custom)?;
        json_value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StateWithSerde {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let json_value = serde_json::Value::deserialize(deserializer)?;
        let json = serde_json::to_string(&json_value).map_err(D::Error::custom)?;
        let value = parse_proto_json(&json).map_err(D::Error::custom)?;
        Ok(Self(value))
    }
}
