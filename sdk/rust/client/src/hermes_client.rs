use {
    crate::{
        CHANNEL_CAPACITY,
        backoff::{PythLazerExponentialBackoff, PythLazerExponentialBackoffBuilder},
        hermes_ws_connection::HermesServerMessageWrapper,
        resilient_hermes_ws_connection::PythLazerResilientHermesWSConnection,
    },
    anyhow::{Result, anyhow, bail},
    backoff::ExponentialBackoff,
    pyth_lazer_protocol::{
        api::Channel,
        hermes::{HermesWsClientMessage, HermesWsServerMessage, PriceFeedMetadata},
        time::FixedRate,
    },
    std::time::Duration,
    tokio::sync::mpsc::{self, error::TrySendError},
    tracing::{Instrument, error, info_span, warn},
    ttl_cache::TtlCache,
    url::Url,
};

const DEDUP_CACHE_SIZE: usize = 100_000;
const DEDUP_TTL: Duration = Duration::from_secs(10);

const WS_URL: &str = "/hermes/ws";
const METADATA_URL: &str = "/hermes/v2/price_feeds";

const DEFAULT_ENDPOINTS: [&str; 3] = [
    "wss://pyth-0.dourolabs.app",
    "wss://pyth-1.dourolabs.app",
    "wss://pyth-2.dourolabs.app",
];

const DEFAULT_NUM_CONNECTIONS: usize = 6;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PythLazerHermesClient {
    endpoints: Vec<Url>,
    access_token: String,
    channel: Channel,
    num_connections: usize,
    connections: Vec<PythLazerResilientHermesWSConnection>,
    backoff: ExponentialBackoff,
    timeout: Duration,
    channel_capacity: usize,
}

impl PythLazerHermesClient {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoints: Vec<Url>,
        access_token: String,
        channel: Channel,
        num_connections: usize,
        backoff: PythLazerExponentialBackoff,
        timeout: Duration,
        channel_capacity: usize,
    ) -> Result<Self> {
        if endpoints.is_empty() {
            bail!("At least one endpoint must be provided");
        }
        Ok(Self {
            endpoints,
            access_token,
            channel,
            num_connections,
            connections: Vec::new(),
            backoff: backoff.into(),
            timeout,
            channel_capacity,
        })
    }

    pub async fn start(&mut self) -> Result<mpsc::Receiver<HermesWsServerMessage>> {
        let (output_sender, output_receiver) =
            mpsc::channel::<HermesWsServerMessage>(self.channel_capacity);
        let (ws_sender, mut ws_receiver) =
            mpsc::channel::<HermesServerMessageWrapper>(CHANNEL_CAPACITY);

        for i in 0..self.num_connections {
            let mut endpoint = self.endpoints[i % self.endpoints.len()].clone();
            endpoint.set_path(WS_URL);

            let connection = PythLazerResilientHermesWSConnection::new(
                endpoint,
                self.access_token.clone(),
                self.channel,
                self.backoff.clone(),
                self.timeout,
                ws_sender.clone(),
            );
            self.connections.push(connection);
        }

        let mut seen_updates = TtlCache::new(DEDUP_CACHE_SIZE);

        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                while let Some(msg) = ws_receiver.recv().await {
                    let cache_key = msg.cache_key();
                    if seen_updates.contains_key(&cache_key) {
                        continue;
                    }
                    seen_updates.insert(cache_key, true, DEDUP_TTL);

                    match output_sender.try_send(msg.0) {
                        Ok(_) => (),
                        Err(TrySendError::Full(r)) => {
                            warn!("Sender channel is full, responses will be delayed");
                            if output_sender.send(r).await.is_err() {
                                error!("Sender channel is closed, stopping client");
                            }
                        }
                        Err(TrySendError::Closed(_)) => {
                            error!("Sender channel is closed, stopping client");
                            break;
                        }
                    }
                }
            }
            .instrument(info_span!("hermes client task", endpoints = ?self.endpoints)),
        );

        Ok(output_receiver)
    }

    pub async fn send_message(&mut self, message: HermesWsClientMessage) -> Result<()> {
        for connection in &mut self.connections {
            connection.send_message(message.clone()).await?;
        }

        Ok(())
    }

    pub async fn fetch_metadata(&self) -> Result<Vec<PriceFeedMetadata>> {
        for endpoint in &self.endpoints {
            let mut metadata_url = endpoint.clone();
            metadata_url.set_path(METADATA_URL);

            if metadata_url.scheme() == "ws" {
                metadata_url
                    .set_scheme("http")
                    .map_err(|_| anyhow!("Unable to set metadata URL scheme to http"))?;
            } else if metadata_url.scheme() == "wss" {
                metadata_url
                    .set_scheme("https")
                    .map_err(|_| anyhow!("Unable to set metadata URL scheme to https"))?;
            };

            let client = reqwest::Client::new();
            let response = match client
                .get(metadata_url.as_str())
                .header("Authorization", format!("Bearer {}", self.access_token))
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(error) => {
                    warn!(
                        ?error,
                        "Failed to fetch metadata from endpoint: {}", metadata_url
                    );
                    continue;
                }
            };

            let feeds: Vec<PriceFeedMetadata> = match response.json().await {
                Ok(feeds) => feeds,
                Err(error) => {
                    warn!(
                        ?error,
                        "Failed to parse metadata from endpoint: {}", metadata_url
                    );
                    continue;
                }
            };

            return Ok(feeds);
        }
        bail!("Failed to fetch metadata from any endpoint");
    }
}

/// Builder for [`PythLazerHermesClient`].
pub struct PythLazerHermesClientBuilder {
    endpoints: Vec<Url>,
    access_token: String,
    channel: Channel,
    num_connections: usize,
    backoff: PythLazerExponentialBackoff,
    timeout: Duration,
    channel_capacity: usize,
}

impl PythLazerHermesClientBuilder {
    pub fn new(access_token: String) -> Self {
        Self {
            endpoints: DEFAULT_ENDPOINTS
                .iter()
                .map(|&s| s.parse().unwrap())
                .collect(),
            access_token,
            channel: Channel::FixedRate(FixedRate::RATE_1000_MS),
            num_connections: DEFAULT_NUM_CONNECTIONS,
            backoff: PythLazerExponentialBackoffBuilder::default().build(),
            timeout: DEFAULT_TIMEOUT,
            channel_capacity: CHANNEL_CAPACITY,
        }
    }

    pub fn with_endpoints(mut self, endpoints: Vec<Url>) -> Self {
        self.endpoints = endpoints;
        self
    }

    pub fn with_channel(mut self, channel: Channel) -> Self {
        self.channel = channel;
        self
    }

    pub fn with_num_connections(mut self, num_connections: usize) -> Self {
        self.num_connections = num_connections;
        self
    }

    pub fn with_backoff(mut self, backoff: PythLazerExponentialBackoff) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_channel_capacity(mut self, channel_capacity: usize) -> Self {
        self.channel_capacity = channel_capacity;
        self
    }

    pub fn build(self) -> Result<PythLazerHermesClient> {
        PythLazerHermesClient::new(
            self.endpoints,
            self.access_token,
            self.channel,
            self.num_connections,
            self.backoff,
            self.timeout,
            self.channel_capacity,
        )
    }
}
