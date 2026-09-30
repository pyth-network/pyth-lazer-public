pub mod packed_evm;
mod standard;
pub use standard::*;

use crate::{
    api::MarketSession,
    price::Price,
    rate::Rate,
    time::{DurationUs, TimestampUs},
    PublisherDatapoint, PublisherId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AggregatedPriceFeedData {
    pub price: Option<Price>,
    pub best_bid_price: Option<Price>,
    pub best_ask_price: Option<Price>,
    pub publisher_count: u16,
    pub exponent: i16,
    pub confidence: Option<Price>,
    pub funding_rate: Option<Rate>,
    pub funding_timestamp: Option<TimestampUs>,
    pub funding_rate_interval: Option<DurationUs>,
    pub market_session: MarketSession,
    pub ema_price: Option<Price>,
    pub ema_confidence: Option<Price>,
    pub feed_update_timestamp: Option<TimestampUs>,
    pub publisher_ids: Vec<PublisherId>,
    pub publisher_data: Vec<PublisherDatapoint>,
}

impl AggregatedPriceFeedData {
    pub fn empty(exponent: i16, market_session: MarketSession, now: TimestampUs) -> Self {
        Self {
            price: None,
            best_bid_price: None,
            best_ask_price: None,
            publisher_count: 0,
            exponent,
            confidence: None,
            funding_rate: None,
            funding_timestamp: None,
            funding_rate_interval: None,
            market_session,
            ema_price: None,
            ema_confidence: None,
            feed_update_timestamp: Some(now),
            publisher_ids: Vec::new(),
            publisher_data: Vec::new(),
        }
    }
}
