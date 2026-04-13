use cached::{Cached, TimedCache};
use derivative::Derivative;
use pyth_lazer_aggregator::price_feed::AtomicStaticData;
use pyth_lazer_aggregator::state::FeedStaticData;
use pyth_lazer_aggregator::state::FeedVisibility;
use pyth_lazer_internal_protocol::entitlement::ConsumerEntitlements;
use pyth_lazer_internal_protocol::entitlement::ConsumerEntitlementsStateAccessLevel;
use pyth_lazer_protocol::{PriceFeedId, SymbolState, api::Channel};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use url::Url;
use {
    anyhow::{Result, bail},
    pyth_lazer_utils::run_every,
};

// No min channel for PYTH as a demo
const FULL_ACCESS_FEED_ID: u32 = 3;

pub const REAL_TIME_CHANNEL_MS: u32 = 1;

#[derive(Deserialize, Derivative, Clone, PartialEq, Eq)]
#[derivative(Debug)]
pub struct EntitlementsConfig {
    pub entitlement_service_urls: Vec<Url>,
    #[serde(default = "default_entitlement_cache_ttl", with = "humantime_serde")]
    pub entitlement_cache_ttl: Duration,
}

pub fn default_entitlement_cache_ttl() -> Duration {
    Duration::from_secs(12 * 60 * 60)
}

#[derive(Debug, Clone)]
pub enum ConsumerEntitlementsKind {
    Normal(Arc<ConsumerEntitlements>),
    AllPublicFeeds(ConsumerEntitlementsStateAccessLevel),
}

/// Error returned when an entitlement check fails, providing detailed context
/// about why the subscription was not allowed.
#[derive(Debug, Clone)]
pub enum EntitlementError {
    /// The requested update rate is faster than the API key allows
    RateViolation {
        requested_channel: String,
        min_allowed_ms: u32,
    },
    /// The requested feed is not accessible with this API key
    FeedNotEntitled {
        feed_id: PriceFeedId,
        hermes_id: Option<String>,
        reason: String,
    },
    /// The feed is not available
    FeedNotAvailable {
        feed_id: PriceFeedId,
        hermes_id: Option<String>,
    },
    FeedDoesNotExist {
        feed_id: PriceFeedId,
        hermes_id: Option<String>,
    },
}

impl std::fmt::Display for EntitlementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntitlementError::RateViolation {
                requested_channel,
                min_allowed_ms,
            } => {
                write!(
                    f,
                    "API key has insufficient access: \
                    requested rate {} is faster than the allowed minimum rate of {}ms",
                    requested_channel, min_allowed_ms
                )
            }
            EntitlementError::FeedNotEntitled {
                feed_id,
                hermes_id: _,
                reason,
            } => {
                write!(
                    f,
                    "API key has insufficient access: \
                    feed {} is not accessible: {}",
                    feed_id.0, reason
                )
            }
            EntitlementError::FeedNotAvailable {
                feed_id,
                hermes_id: _,
            } => {
                write!(
                    f,
                    "API key has insufficient access: \
                    feed {} is not available",
                    feed_id.0
                )
            }
            EntitlementError::FeedDoesNotExist {
                feed_id,
                hermes_id: _,
            } => {
                write!(
                    f,
                    "API key has insufficient access: \
                    feed {} does not exist",
                    feed_id.0
                )
            }
        }
    }
}

pub struct EntitlementsServiceClient {
    pub client: Client,
    pub urls: Vec<Url>,
    pub cache: RwLock<TimedCache<String, Arc<ConsumerEntitlements>>>,
}

impl EntitlementsServiceClient {
    async fn fetch_entitlements_from_all_instances(
        &self,
        consumer_token: &str,
    ) -> Result<ConsumerEntitlements> {
        for url in self.urls.iter() {
            match self
                .fetch_entitlements_from_single_instance(url, consumer_token)
                .await
            {
                Ok(entitlements_response) => return Ok(entitlements_response),
                Err(e) => {
                    tracing::warn!("Failed to fetch entitlements from service url: {url}: {e:?}")
                }
            }
        }
        bail!("All entitlement-service instance queries failed")
    }

    async fn fetch_entitlements_from_single_instance(
        &self,
        url: &Url,
        consumer_token: &str,
    ) -> Result<ConsumerEntitlements> {
        let response = self
            .client
            .post(url.clone())
            .json(&json!({"api_key": consumer_token}))
            .send()
            .await?;
        match response.status() {
            StatusCode::OK => (),
            StatusCode::NOT_FOUND => {
                // TODO: Perhaps warn here on unrecognized token?
                return Ok(ConsumerEntitlements::no_access());
            }
            status => match response.text().await {
                Ok(body) => bail!("get_entitlements error response status: {status} body: {body}"),
                Err(_) => bail!("get_entitlements error response status: {status}"),
            },
        }
        Ok(response.json::<ConsumerEntitlements>().await?)
    }
}

#[derive(Derivative)]
pub struct EntitlementManager {
    #[derivative(Debug = "ignore")]
    pub static_data: AtomicStaticData,
    pub entitlement_service_client: Option<EntitlementsServiceClient>,
    pub legacy_config_tokens: HashSet<String>,
}

impl EntitlementManager {
    pub fn new(
        config: Option<EntitlementsConfig>,
        static_data: AtomicStaticData,
        legacy_config_tokens: &[String],
    ) -> Self {
        Self {
            static_data,
            entitlement_service_client: config.map(
                |EntitlementsConfig {
                     entitlement_service_urls,
                     entitlement_cache_ttl,
                 }| EntitlementsServiceClient {
                    client: Client::new(),
                    urls: entitlement_service_urls,
                    cache: RwLock::new(TimedCache::with_lifespan(entitlement_cache_ttl)),
                },
            ),
            legacy_config_tokens: legacy_config_tokens.iter().cloned().collect(),
        }
    }

    // TODO: For now, we're letting the PYTH feed be accessible for all channels for free tier
    // users, for demo purposes. We will come up with a more general mechanism in the future.
    fn is_full_access_feed_id(price_feed_id: PriceFeedId) -> bool {
        price_feed_id.0 == FULL_ACCESS_FEED_ID
    }

    /// Check entitlements and return a simple boolean.
    /// Use `check_entitlement` for detailed error information.
    #[cfg(test)]
    pub(crate) fn is_entitled(
        &self,
        entitlements: &ConsumerEntitlementsKind,
        price_feed_ids: &[PriceFeedId],
        channel: Channel,
    ) -> bool {
        self.check_entitlement(entitlements, price_feed_ids, channel)
            .is_ok()
    }

    /// Check entitlements and return detailed error information on failure.
    pub fn check_entitlement(
        &self,
        entitlements: &ConsumerEntitlementsKind,
        price_feed_ids: &[PriceFeedId],
        channel: Channel,
    ) -> Result<(), EntitlementError> {
        let feed_errors = self.check_entitlements_per_feed(entitlements, price_feed_ids, channel);
        if let Some((_, error)) = feed_errors.into_iter().next() {
            return Err(error);
        }
        Ok(())
    }

    /// Check entitlements per feed, returning per-feed failures separately
    pub fn check_entitlements_per_feed(
        &self,
        entitlements: &ConsumerEntitlementsKind,
        price_feed_ids: &[PriceFeedId],
        channel: Channel,
    ) -> Vec<(PriceFeedId, EntitlementError)> {
        let mut feed_errors = Vec::new();
        match entitlements {
            ConsumerEntitlementsKind::Normal(entitlements) => {
                let static_data = self.static_data.load();
                let violates_min_channel = match channel {
                    Channel::FixedRate(fixed_rate) => {
                        u64::from(entitlements.min_channel_ms) > fixed_rate.duration().as_millis()
                    }
                    Channel::RealTime => entitlements.min_channel_ms > REAL_TIME_CHANNEL_MS,
                };
                for price_feed_id in price_feed_ids {
                    if violates_min_channel && !Self::is_full_access_feed_id(*price_feed_id) {
                        feed_errors.push((
                            *price_feed_id,
                            EntitlementError::RateViolation {
                                requested_channel: channel.to_string(),
                                min_allowed_ms: entitlements.min_channel_ms,
                            },
                        ));
                        continue;
                    }
                    let Some(feed) = static_data.feeds.get(price_feed_id) else {
                        feed_errors.push((
                            *price_feed_id,
                            EntitlementError::FeedDoesNotExist {
                                feed_id: *price_feed_id,
                                hermes_id: None,
                            },
                        ));
                        continue;
                    };
                    if let Err(error) = Self::check_feed_entitlement(feed, entitlements) {
                        feed_errors.push((*price_feed_id, error));
                    }
                }
            }
            ConsumerEntitlementsKind::AllPublicFeeds(_) => {
                let static_data = self.static_data.load();
                for price_feed_id in price_feed_ids {
                    if let Some(feed) = static_data.feeds.get(price_feed_id) {
                        match feed.visibility() {
                            FeedVisibility::Public => {}
                            FeedVisibility::Unlisted => {
                                feed_errors.push((
                                    *price_feed_id,
                                    EntitlementError::FeedNotAvailable {
                                        feed_id: feed.id,
                                        hermes_id: feed.hermes_id().map(|id| id.to_string()),
                                    },
                                ));
                            }
                        }
                    }
                }
            }
        }

        feed_errors
    }

    pub fn has_access_for_state(
        entitlements: &ConsumerEntitlementsKind,
        feed: &FeedStaticData,
    ) -> bool {
        match feed.state {
            SymbolState::Stable => true,
            SymbolState::ComingSoon | SymbolState::Inactive => false,
            SymbolState::Beta => Self::has_beta_access(entitlements),
        }
    }

    // If there is an API failure, return full access.
    // If entitlement-service doesn't recognize this token, return no access.
    // If it is recognized, return its entitlements.
    pub async fn get_consumer_entitlements(&self, token: &str) -> ConsumerEntitlementsKind {
        // TODO: For now, give hardcoded tokens full access to public feeds.
        // Eventually they will be migrated to the entitlement system as appropriate.
        // No access to beta feeds for legacy tokens for now
        if self.legacy_config_tokens.contains(token) {
            return ConsumerEntitlementsKind::AllPublicFeeds(
                ConsumerEntitlementsStateAccessLevel::Stable,
            );
        }

        if let Some(entitlement_service_client) = &self.entitlement_service_client {
            // Need to drop read guard to get the write lock inside the match.
            let entitlement_cache_value = {
                let mut guard = entitlement_service_client.cache.write().await;
                guard.cache_get(token).cloned()
            };
            match entitlement_cache_value {
                Some(entitlements) => ConsumerEntitlementsKind::Normal(entitlements),
                None => {
                    match entitlement_service_client
                        .fetch_entitlements_from_all_instances(token)
                        .await
                    {
                        Ok(entitlements_response) => {
                            let entitlements_response = Arc::new(entitlements_response);
                            entitlement_service_client
                                .cache
                                .write()
                                .await
                                .cache_set(token.to_string(), entitlements_response.clone());
                            ConsumerEntitlementsKind::Normal(entitlements_response)
                        }
                        // Then we currently have no connection to any entitlement-service and so will
                        // give access ("fail open") to avoid an outage for valid consumers. This applies to beta feeds as well.
                        Err(error) => {
                            run_every!(15 secs, {
                                tracing::error!(
                                    ?error,
                                    "fetch_entitlements_from_entitlements_service error \
                                    querying all entitlement-service instances; allowing full access",
                                )
                            });
                            ConsumerEntitlementsKind::AllPublicFeeds(
                                ConsumerEntitlementsStateAccessLevel::Beta,
                            )
                        }
                    }
                }
            }
        } else {
            ConsumerEntitlementsKind::Normal(Arc::new(ConsumerEntitlements::no_access()))
        }
    }

    fn has_beta_access(entitlements: &ConsumerEntitlementsKind) -> bool {
        match entitlements {
            ConsumerEntitlementsKind::Normal(entitlements) => {
                matches!(
                    entitlements.state_access_level,
                    ConsumerEntitlementsStateAccessLevel::Beta
                )
            }
            ConsumerEntitlementsKind::AllPublicFeeds(state_access_level) => {
                match state_access_level {
                    ConsumerEntitlementsStateAccessLevel::Stable => false,
                    ConsumerEntitlementsStateAccessLevel::Beta => true,
                }
            }
        }
    }

    fn check_feed_entitlement(
        feed: &FeedStaticData,
        entitlements: &ConsumerEntitlements,
    ) -> Result<(), EntitlementError> {
        // accept if explicitly included
        if entitlements.include_feed_ids.contains(&feed.id.0) {
            return Ok(());
        }

        // otherwise, see if it's in enabled asset types
        if entitlements.asset_types.contains(feed.asset_type()) {
            return Ok(());
        }

        // Not entitled - provide detailed error
        let allowed_types: Vec<_> = entitlements.asset_types.iter().cloned().collect();
        Err(EntitlementError::FeedNotEntitled {
            feed_id: feed.id,
            hermes_id: feed.hermes_id().map(|id| id.to_string()),
            reason: format!(
                "asset type '{}' is not in allowed types: {:?}",
                feed.asset_type(),
                allowed_types
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::entitlement_manager::{EntitlementManager, FULL_ACCESS_FEED_ID};
    use axum::{Router, extract::Json, response::IntoResponse, routing::post};
    use cached::Cached;
    use pyth_lazer_aggregator::price_feed::AtomicStaticData;
    use pyth_lazer_aggregator::state::{FeedStaticData, State};
    use pyth_lazer_internal_protocol::SequenceNo;
    use pyth_lazer_internal_protocol::entitlement::{
        ConsumerEntitlements, ConsumerEntitlementsRequest, ConsumerEntitlementsStateAccessLevel,
    };
    use pyth_lazer_internal_protocol::market_schedule::{
        MarketSessionProperties, MarketSessionSchedule,
    };
    use pyth_lazer_protocol::time::DurationUs;
    use pyth_lazer_protocol::{DynamicValue, FeedKind, SymbolState};
    use pyth_lazer_protocol::{PriceFeedId, api::Channel, time::FixedRate};
    use std::collections::{BTreeMap, HashSet};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use url::Url;
    use {
        super::{EntitlementsConfig, default_entitlement_cache_ttl},
        pyth_lazer_utils::task::spawn,
    };

    const FREE_KEY: &str = "free_key";
    const PRO_KEY: &str = "pro_key";
    const CUSTOM_KEY: &str = "custom_key";
    const BAD_KEY: &str = "bad_key";
    const FIFTY_MS_KEY: &str = "fifty_ms_key";
    const TWO_HUNDRED_MS_KEY: &str = "two_hundred_ms_key";
    const BETA_KEY: &str = "beta_key";
    const HARDCODED_LEGACY_TOKEN: &str = "hardcoded_token";

    fn generate_feeds_static_data() -> BTreeMap<PriceFeedId, FeedStaticData> {
        BTreeMap::from([
            (
                PriceFeedId(1),
                FeedStaticData {
                    id: PriceFeedId(1),
                    metadata: BTreeMap::<String, DynamicValue>::from([
                        ("asset_type".to_string(), "crypto".to_string().into()),
                        ("name".to_string(), "BTCUSD".to_string().into()),
                        (
                            "description".to_string(),
                            "BITCOIN / US DOLLAR".to_string().into(),
                        ),
                    ]),
                    symbol: "Crypto.BTC/USD".to_string(),
                    exponent: -8,
                    min_channel: Channel::FixedRate(FixedRate::MIN),
                    expiry_time: DurationUs::from_secs(5).expect("Invalid duration"),
                    market_sessions: vec![
                        MarketSessionProperties::regular_from_market_session_schedule(
                            MarketSessionSchedule::default(),
                        ),
                    ],
                    corporate_actions: vec![],
                    default_min_publishers: 1,
                    state: SymbolState::Stable,
                    kind: FeedKind::Price,
                    is_enabled_in_shard: true,
                    enable_in_shard_timestamp: None,
                    disable_in_shard_timestamp: None,
                    exchange_id: None,
                },
            ),
            (
                PriceFeedId(112),
                FeedStaticData {
                    id: PriceFeedId(112),
                    metadata: BTreeMap::<String, DynamicValue>::from([
                        ("asset_type".to_string(), "funding-rate".to_string().into()),
                        ("name".to_string(), "BTCUSDT".to_string().into()),
                        (
                            "description".to_string(),
                            "Binance BTC / USDT Funding Rate".to_string().into(),
                        ),
                    ]),
                    symbol: "FundingRate.Binance.BTC/USDT".to_string(),
                    exponent: -12,
                    min_channel: Channel::FixedRate(FixedRate::RATE_200_MS),
                    expiry_time: DurationUs::from_secs(5).expect("Invalid duration"),
                    market_sessions: vec![
                        MarketSessionProperties::regular_from_market_session_schedule(
                            MarketSessionSchedule::default(),
                        ),
                    ],
                    corporate_actions: vec![],
                    default_min_publishers: 0,
                    state: SymbolState::Stable,
                    kind: FeedKind::FundingRate,
                    is_enabled_in_shard: true,
                    enable_in_shard_timestamp: None,
                    disable_in_shard_timestamp: None,
                    exchange_id: None,
                },
            ),
            (
                PriceFeedId(922),
                FeedStaticData {
                    id: PriceFeedId(922),
                    metadata: BTreeMap::<String, DynamicValue>::from([
                        ("asset_type".to_string(), "equity".to_string().into()),
                        ("name".to_string(), "AAPL".to_string().into()),
                        (
                            "description".to_string(),
                            "APPLE INC / US DOLLAR".to_string().into(),
                        ),
                    ]),
                    symbol: "Equity.US.AAPL/USD".to_string(),
                    exponent: -5,
                    min_channel: Channel::FixedRate(FixedRate::RATE_200_MS),
                    expiry_time: DurationUs::from_secs(5).expect("Invalid duration"),
                    market_sessions: vec![
                        MarketSessionProperties::regular_from_market_session_schedule(
                            MarketSessionSchedule::default(),
                        ),
                    ],
                    corporate_actions: vec![],
                    default_min_publishers: 0,
                    state: SymbolState::Stable,
                    kind: FeedKind::Price,
                    is_enabled_in_shard: true,
                    enable_in_shard_timestamp: None,
                    disable_in_shard_timestamp: None,
                    exchange_id: None,
                },
            ),
            (
                PriceFeedId(10002),
                FeedStaticData {
                    id: PriceFeedId(10002),
                    metadata: BTreeMap::<String, DynamicValue>::from([
                        ("asset_type".to_string(), "crypto".to_string().into()),
                        ("name".to_string(), "BETACOIN".to_string().into()),
                        (
                            "description".to_string(),
                            "BETACOIN / US DOLLAR".to_string().into(),
                        ),
                    ]),
                    symbol: "Crypto.BETACOIN/USD".to_string(),
                    exponent: -8,
                    min_channel: Channel::FixedRate(FixedRate::RATE_200_MS),
                    expiry_time: DurationUs::from_secs(5).expect("Invalid duration"),
                    market_sessions: vec![
                        MarketSessionProperties::regular_from_market_session_schedule(
                            MarketSessionSchedule::default(),
                        ),
                    ],
                    default_min_publishers: 1,
                    state: SymbolState::Beta,
                    kind: FeedKind::Price,
                    is_enabled_in_shard: true,
                    enable_in_shard_timestamp: None,
                    disable_in_shard_timestamp: None,
                    exchange_id: None,
                    corporate_actions: vec![],
                },
            ),
            (
                PriceFeedId(FULL_ACCESS_FEED_ID),
                FeedStaticData {
                    id: PriceFeedId(FULL_ACCESS_FEED_ID),
                    metadata: BTreeMap::<String, DynamicValue>::from([
                        ("asset_type".to_string(), "crypto".to_string().into()),
                        ("name".to_string(), "PYTHUSD".to_string().into()),
                        (
                            "description".to_string(),
                            "PYTH / US DOLLAR".to_string().into(),
                        ),
                    ]),
                    symbol: "Crypto.PYTH/USD".to_string(),
                    exponent: -8,
                    min_channel: Channel::FixedRate(FixedRate::MIN),
                    expiry_time: DurationUs::from_secs(5).expect("Invalid duration"),
                    market_sessions: vec![
                        MarketSessionProperties::regular_from_market_session_schedule(
                            MarketSessionSchedule::default(),
                        ),
                    ],
                    default_min_publishers: 1,
                    state: SymbolState::Stable,
                    kind: FeedKind::Price,
                    is_enabled_in_shard: true,
                    enable_in_shard_timestamp: None,
                    disable_in_shard_timestamp: None,
                    exchange_id: None,
                    corporate_actions: vec![],
                },
            ),
        ])
    }

    async fn get_feeds_map() -> AtomicStaticData {
        let now = pyth_lazer_protocol::time::TimestampUs::now();
        let state = State::init_for_test(None, generate_feeds_static_data(), now, SequenceNo(0));
        AtomicStaticData::from_const(state.static_data)
    }

    async fn mock_entitlement_service() -> (Url, tokio::task::JoinHandle<()>) {
        let app = Router::new().route("/entitlements", post(mock_entitlements_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = Url::parse(&format!("http://{addr}/entitlements")).unwrap();
        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        let handle = spawn("mock entitlement service", async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, handle)
    }

    async fn mock_entitlement_service_down() -> Url {
        // Bind to a port and immediately drop it to ensure it's closed
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        Url::parse(&format!("http://{addr}/entitlements")).unwrap()
    }

    async fn mock_entitlements_handler(
        Json(request): Json<ConsumerEntitlementsRequest>,
    ) -> impl IntoResponse {
        match request.api_key.as_str() {
            FREE_KEY => (
                axum::http::StatusCode::OK,
                Json(ConsumerEntitlements {
                    asset_types: vec!["crypto".to_string()].into_iter().collect(),
                    min_channel_ms: 1000,
                    include_feed_ids: HashSet::new(),
                    state_access_level: Default::default(),
                }),
            )
                .into_response(),
            PRO_KEY => (
                axum::http::StatusCode::OK,
                Json(ConsumerEntitlements {
                    asset_types: vec![
                        "crypto".to_string(),
                        "funding_rate".to_string(),
                        "equity".to_string(),
                    ]
                    .into_iter()
                    .collect(),
                    min_channel_ms: 1,
                    include_feed_ids: HashSet::new(),
                    state_access_level: Default::default(),
                }),
            )
                .into_response(),
            CUSTOM_KEY => (
                axum::http::StatusCode::OK,
                Json(ConsumerEntitlements {
                    asset_types: vec!["crypto".to_string()].into_iter().collect(),
                    min_channel_ms: 1,
                    include_feed_ids: vec![922].into_iter().collect(),
                    state_access_level: Default::default(),
                }),
            )
                .into_response(),
            FIFTY_MS_KEY => (
                axum::http::StatusCode::OK,
                Json(ConsumerEntitlements {
                    asset_types: vec!["crypto".to_string()].into_iter().collect(),
                    min_channel_ms: 50,
                    include_feed_ids: Default::default(),
                    state_access_level: Default::default(),
                }),
            )
                .into_response(),
            TWO_HUNDRED_MS_KEY => (
                axum::http::StatusCode::OK,
                Json(ConsumerEntitlements {
                    asset_types: vec!["crypto".to_string()].into_iter().collect(),
                    min_channel_ms: 200,
                    include_feed_ids: Default::default(),
                    state_access_level: Default::default(),
                }),
            )
                .into_response(),
            BETA_KEY => (
                axum::http::StatusCode::OK,
                Json(ConsumerEntitlements {
                    asset_types: vec!["crypto".to_string()].into_iter().collect(),
                    min_channel_ms: 200,
                    include_feed_ids: Default::default(),
                    state_access_level: ConsumerEntitlementsStateAccessLevel::Beta,
                }),
            )
                .into_response(),
            _ => axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }

    #[tokio::test]
    async fn test_free_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(FREE_KEY)
            .await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_pro_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager.get_consumer_entitlements(PRO_KEY).await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::RealTime,
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_custom_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(CUSTOM_KEY)
            .await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::RealTime,
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(112)],
            Channel::RealTime,
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::RealTime,
        ));
    }

    #[tokio::test]
    async fn test_fifty_ms_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(FIFTY_MS_KEY)
            .await;

        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::RealTime,
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));
    }

    #[tokio::test]
    async fn test_two_hundred_ms_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(TWO_HUNDRED_MS_KEY)
            .await;

        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::RealTime,
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));
    }

    #[tokio::test]
    async fn test_bad_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager.get_consumer_entitlements(BAD_KEY).await;

        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_entitlement_service_down_fail_open() {
        let url = mock_entitlement_service_down().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager.get_consumer_entitlements(BAD_KEY).await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_entitlement_service_all_instances_down_fail_open() {
        let url_down_1 = mock_entitlement_service_down().await;
        let url_down_2 = mock_entitlement_service_down().await;
        let url_down_3 = mock_entitlement_service_down().await;
        let urls = vec![url_down_1, url_down_2, url_down_3];
        let config = EntitlementsConfig {
            entitlement_service_urls: urls,
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager.get_consumer_entitlements(BAD_KEY).await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_free_key_with_some_instances_down() {
        let url_down_1 = mock_entitlement_service_down().await;
        let url_down_2 = mock_entitlement_service_down().await;
        let (url_up_3, _handle) = mock_entitlement_service().await;
        let urls = vec![url_down_1, url_down_2, url_up_3];
        let config = EntitlementsConfig {
            entitlement_service_urls: urls,
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };

        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(FREE_KEY)
            .await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_entitlements_on_hardcoded_token() {
        let url = mock_entitlement_service_down().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(
            Some(config),
            get_feeds_map().await,
            &[HARDCODED_LEGACY_TOKEN.to_string()],
        );

        let entitlements = entitlement_manager
            .get_consumer_entitlements(HARDCODED_LEGACY_TOKEN)
            .await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_entitlements_without_entitlement_service() {
        let entitlement_manager = EntitlementManager::new(
            None,
            get_feeds_map().await,
            &[HARDCODED_LEGACY_TOKEN.to_string()],
        );

        let entitlements = entitlement_manager
            .get_consumer_entitlements(HARDCODED_LEGACY_TOKEN)
            .await;

        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_bad_key_without_entitlement_service() {
        let entitlement_manager = EntitlementManager::new(
            None,
            get_feeds_map().await,
            &[HARDCODED_LEGACY_TOKEN.to_string()],
        );

        let entitlements = entitlement_manager.get_consumer_entitlements(BAD_KEY).await;

        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1), PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
    }

    #[tokio::test]
    async fn test_cache_expires_after_ttl() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: Duration::from_secs(1),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let _entitlements = entitlement_manager
            .get_consumer_entitlements(FREE_KEY)
            .await;

        assert!(
            entitlement_manager
                .entitlement_service_client
                .as_ref()
                .unwrap()
                .cache
                .write()
                .await
                .cache_get(FREE_KEY)
                .is_some()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            entitlement_manager
                .entitlement_service_client
                .as_ref()
                .unwrap()
                .cache
                .write()
                .await
                .cache_get(FREE_KEY)
                .is_some()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            entitlement_manager
                .entitlement_service_client
                .as_ref()
                .unwrap()
                .cache
                .write()
                .await
                .cache_get(FREE_KEY)
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_free_key_with_full_access_feed() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(FREE_KEY)
            .await;

        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::RealTime
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(FULL_ACCESS_FEED_ID)],
            Channel::RealTime
        ));
    }

    #[tokio::test]
    async fn test_check_entitlement_rate_violation_error() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(TWO_HUNDRED_MS_KEY)
            .await;

        // Request faster rate than allowed - should get RateViolation error
        let result = entitlement_manager.check_entitlement(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("insufficient access"));
        assert!(err_msg.contains("fixed_rate@50ms"));
        assert!(err_msg.contains("200ms"));
    }

    #[tokio::test]
    async fn test_check_entitlement_feed_not_entitled_error() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(FREE_KEY)
            .await;

        // Request equity feed with crypto-only key - should get FeedNotEntitled error
        let result = entitlement_manager.check_entitlement(
            &entitlements,
            &[PriceFeedId(922)], // Equity feed (AAPL)
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("insufficient access"));
        assert!(err_msg.contains("922"));
        assert!(err_msg.contains("equity"));
        assert!(err_msg.contains("crypto"));
    }

    #[tokio::test]
    async fn test_beta_key() {
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url.clone()],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(BETA_KEY)
            .await;

        // Beta key has crypto asset_type and min_channel 200ms — same rate/asset rules apply
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::FixedRate(FixedRate::RATE_50_MS),
        ));
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(1)],
            Channel::RealTime,
        ));
        // Beta key should not be entitled to non-crypto feeds
        assert!(!entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(922)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));

        // Beta key CAN access a beta-state crypto feed
        assert!(entitlement_manager.is_entitled(
            &entitlements,
            &[PriceFeedId(10002)],
            Channel::FixedRate(FixedRate::RATE_200_MS),
        ));

        // Non-beta (free) key passes entitlement check for beta feed
        // (state access is checked separately by the router/HTTP layer via has_access_for_state)
        let config2 = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager2 =
            EntitlementManager::new(Some(config2), get_feeds_map().await, &[]);
        let free_entitlements = entitlement_manager2
            .get_consumer_entitlements(FREE_KEY)
            .await;
        assert!(entitlement_manager2.is_entitled(
            &free_entitlements,
            &[PriceFeedId(10002)],
            Channel::FixedRate(FixedRate::RATE_1000_MS),
        ));

        // But has_access_for_state rejects it for non-beta consumers
        let beta_feed = generate_feeds_static_data()
            .into_iter()
            .find(|(id, _)| *id == PriceFeedId(10002))
            .unwrap()
            .1;
        assert!(!EntitlementManager::has_access_for_state(
            &free_entitlements,
            &beta_feed,
        ));
        assert!(EntitlementManager::has_access_for_state(
            &entitlements,
            &beta_feed,
        ));
    }

    #[tokio::test]
    async fn test_include_feed_ids_overrides_asset_type() {
        // CUSTOM_KEY has asset_types: ["crypto"] but include_feed_ids: [922]
        // Feed 922 is an equity feed - should still be entitled via include_feed_ids
        let (url, _handle) = mock_entitlement_service().await;
        let config = EntitlementsConfig {
            entitlement_service_urls: vec![url],
            entitlement_cache_ttl: default_entitlement_cache_ttl(),
        };
        let entitlement_manager = EntitlementManager::new(Some(config), get_feeds_map().await, &[]);
        let entitlements = entitlement_manager
            .get_consumer_entitlements(CUSTOM_KEY)
            .await;

        // Feed 922 (equity) should be entitled because it's in include_feed_ids,
        // even though "equity" is not in asset_types (only "crypto" is)
        assert!(
            entitlement_manager.is_entitled(
                &entitlements,
                &[PriceFeedId(922)],
                Channel::FixedRate(FixedRate::RATE_200_MS),
            ),
            "Feed 922 should be entitled via include_feed_ids despite being equity asset type"
        );

        // Feed 112 (funding-rate) should NOT be entitled because:
        // - It's not in include_feed_ids
        // - Its asset type "funding-rate" is not in asset_types ["crypto"]
        assert!(
            !entitlement_manager.is_entitled(
                &entitlements,
                &[PriceFeedId(112)],
                Channel::FixedRate(FixedRate::RATE_200_MS),
            ),
            "Feed 112 should not be entitled - not in include_feed_ids and wrong asset type"
        );

        // Feed 1 (crypto) should be entitled via asset_types
        assert!(
            entitlement_manager.is_entitled(
                &entitlements,
                &[PriceFeedId(1)],
                Channel::FixedRate(FixedRate::RATE_200_MS),
            ),
            "Feed 1 should be entitled via asset_types containing 'crypto'"
        );
    }
}
