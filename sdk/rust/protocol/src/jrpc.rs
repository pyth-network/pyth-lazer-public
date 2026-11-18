use crate::api::{MarketSession, TradingStatus};
use crate::rate::Rate;
use crate::symbol_state::SymbolState;
use crate::time::TimestampUs;
use crate::PriceFeedId;
use crate::{api::Channel, price::Price};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone, Debug, Default, Eq, PartialEq)]
#[serde(untagged)]
pub enum JrpcId {
    String(String),
    Int(i64),
    #[default]
    Null,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct PythLazerAgentJrpcV1 {
    pub jsonrpc: JsonRpcVersion,
    #[serde(flatten)]
    pub params: JrpcCall,
    #[serde(default)]
    pub id: JrpcId,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(tag = "method", content = "params")]
#[serde(rename_all = "snake_case")]
pub enum JrpcCall {
    PushUpdate(FeedUpdateParams),
    PushUpdates(Vec<FeedUpdateParams>),
    GetMetadata(GetMetadataParams),
}

/// Names one feed by an instrument identifier. The tag matches the feed metadata key and
/// each scheme carries the fields that make it name exactly one feed.
#[derive(Serialize, Debug, Eq, PartialEq, Clone, Hash)]
#[serde(tag = "scheme", content = "value", rename_all = "snake_case")]
pub enum Identifier {
    Cusip(Cusip),
    /// A scheme introduced after this build; never routes.
    Unknown,
}

/// Hand-written so an unknown scheme parses as [`Identifier::Unknown`] instead of failing the
/// whole request; serde's `other` only fits unit variants. Scheme names mirror the `Serialize`.
impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = serde_json::Value::deserialize(deserializer)?;
        match raw.get("scheme").and_then(serde_json::Value::as_str) {
            Some("cusip") => Cusip::deserialize(&raw["value"])
                .map(Self::Cusip)
                .map_err(D::Error::custom),
            Some(_) => Ok(Self::Unknown),
            None => Err(D::Error::missing_field("scheme")),
        }
    }
}

/// A Treasury CUSIP quoted in `%` is its yield feed, quoted in `USD` its price feed.
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Hash)]
pub struct Cusip {
    pub cusip: String,
    pub quote_currency: String,
}

/// At least one of `feed_id` and `identifier` is required. Without `feed_id`, the agent
/// routes the update to every feed whose metadata matches `identifier`. With both, `feed_id`
/// is used as is and the identifier is not checked against metadata yet.
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct FeedUpdateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_id: Option<PriceFeedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<Identifier>,
    pub source_timestamp: TimestampUs,
    pub update: UpdateParams,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
#[serde(tag = "type")]
pub enum UpdateParams {
    #[serde(rename = "price")]
    PriceUpdate {
        price: Option<Price>,
        best_bid_price: Option<Price>,
        best_ask_price: Option<Price>,
        trading_status: Option<TradingStatus>,
        market_session: Option<MarketSession>,
    },
    #[serde(rename = "funding_rate")]
    FundingRateUpdate {
        price: Option<Price>,
        rate: Rate,
        #[serde(default = "default_funding_rate_interval", with = "humantime_serde")]
        funding_rate_interval: Option<Duration>,
    },
}

/// A feed update whose target feed is known: what the agent sends on after resolving
/// a [`FeedUpdateParams`].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct ResolvedFeedUpdate {
    pub feed_id: PriceFeedId,
    pub identifier: Option<Identifier>,
    pub source_timestamp: TimestampUs,
    pub update: UpdateParams,
}

impl From<ResolvedFeedUpdate> for FeedUpdateParams {
    fn from(value: ResolvedFeedUpdate) -> Self {
        Self {
            feed_id: Some(value.feed_id),
            identifier: value.identifier,
            source_timestamp: value.source_timestamp,
            update: value.update,
        }
    }
}

impl Identifier {
    /// `quote_currency` is `"%"` for a Treasury yield feed and `"USD"` for its price feed.
    pub fn cusip(cusip: impl Into<String>, quote_currency: impl Into<String>) -> Self {
        Self::Cusip(Cusip {
            cusip: cusip.into(),
            quote_currency: quote_currency.into(),
        })
    }

    /// Trimmed and upper-cased, so a CUSIP and its currency match however they were typed.
    pub fn normalized(&self) -> Self {
        match self {
            Self::Cusip(cusip) => Self::Cusip(Cusip {
                cusip: cusip.cusip.trim().to_ascii_uppercase(),
                quote_currency: cusip.quote_currency.trim().to_ascii_uppercase(),
            }),
            Self::Unknown => Self::Unknown,
        }
    }
}

impl FeedUpdateParams {
    pub fn by_feed_id(
        feed_id: PriceFeedId,
        source_timestamp: TimestampUs,
        update: UpdateParams,
    ) -> Self {
        Self {
            feed_id: Some(feed_id),
            identifier: None,
            source_timestamp,
            update,
        }
    }

    /// Routed by the agent to every feed whose metadata matches `identifier`.
    pub fn by_identifier(
        identifier: Identifier,
        source_timestamp: TimestampUs,
        update: UpdateParams,
    ) -> Self {
        Self {
            feed_id: None,
            identifier: Some(identifier),
            source_timestamp,
            update,
        }
    }

    /// `quote_currency` is `"%"` for a Treasury yield feed and `"USD"` for its price feed.
    pub fn by_cusip(
        cusip: impl Into<String>,
        quote_currency: impl Into<String>,
        source_timestamp: TimestampUs,
        update: UpdateParams,
    ) -> Self {
        Self::by_identifier(
            Identifier::cusip(cusip, quote_currency),
            source_timestamp,
            update,
        )
    }

    /// Attach the instrument priced to a `feed_id` update.
    pub fn with_identifier(mut self, identifier: Identifier) -> Self {
        self.identifier = Some(identifier);
        self
    }

    pub fn resolve(self, feed_id: PriceFeedId) -> ResolvedFeedUpdate {
        ResolvedFeedUpdate {
            feed_id,
            identifier: self.identifier,
            source_timestamp: self.source_timestamp,
            update: self.update,
        }
    }
}

impl UpdateParams {
    pub fn price(
        price: Option<Price>,
        best_bid_price: Option<Price>,
        best_ask_price: Option<Price>,
    ) -> Self {
        Self::PriceUpdate {
            price,
            best_bid_price,
            best_ask_price,
            trading_status: None,
            market_session: None,
        }
    }
}

fn default_funding_rate_interval() -> Option<Duration> {
    None
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Filter {
    pub name: Option<String>,
    pub asset_type: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct GetMetadataParams {
    pub names: Option<Vec<String>>,
    pub asset_types: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(untagged)]
pub enum JrpcResponse<T> {
    Success(JrpcSuccessResponse<T>),
    Error(JrpcErrorResponse),
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct JrpcSuccessResponse<T> {
    pub jsonrpc: JsonRpcVersion,
    pub result: T,
    pub id: JrpcId,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct JrpcErrorResponse {
    pub jsonrpc: JsonRpcVersion,
    pub error: JrpcErrorObject,
    pub id: JrpcId,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct JrpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum JrpcError {
    ParseError(String),
    InvalidParams(String),
    InternalError(String),
    SendUpdateError(FeedUpdateParams),
}

// note: error codes can be found in the rfc https://www.jsonrpc.org/specification#error_object
impl From<JrpcError> for JrpcErrorObject {
    fn from(error: JrpcError) -> Self {
        match error {
            JrpcError::ParseError(error_message) => JrpcErrorObject {
                code: -32700,
                message: "Parse error".to_string(),
                data: Some(error_message.into()),
            },
            JrpcError::InvalidParams(error_message) => JrpcErrorObject {
                code: -32602,
                message: "Invalid params".to_string(),
                data: Some(error_message.into()),
            },
            JrpcError::InternalError(error_message) => JrpcErrorObject {
                code: -32603,
                message: "Internal error".to_string(),
                data: Some(error_message.into()),
            },
            JrpcError::SendUpdateError(feed_update_params) => JrpcErrorObject {
                code: -32000,
                message: "Internal error".to_string(),
                data: Some(serde_json::to_value(feed_update_params).unwrap()),
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SymbolMetadata {
    pub pyth_lazer_id: PriceFeedId,
    pub name: String,
    pub symbol: String,
    pub description: String,
    pub asset_type: String,
    pub exponent: i16,
    pub cmc_id: Option<u32>,
    #[serde(default, with = "humantime_serde", alias = "interval")]
    pub funding_rate_interval: Option<Duration>,
    pub min_publishers: u16,
    pub min_channel: Channel,
    pub state: SymbolState,
    pub hermes_id: Option<String>,
    pub quote_currency: Option<String>,
    pub nasdaq_symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cusip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auction_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maturity_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_auction_date: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jrpc::JrpcCall::{GetMetadata, PushUpdate};

    /// Clients (agent, client SDK) deserialize the symbols listing with this
    /// type; a state introduced after their build must parse as `Unknown`
    /// instead of failing the whole response.
    #[test]
    fn symbol_metadata_with_future_state_parses_as_unknown() {
        let json = r#"
        {
          "pyth_lazer_id": 1,
          "name": "BTCUSD",
          "symbol": "Crypto.BTC/USD",
          "description": "BITCOIN / US DOLLAR",
          "asset_type": "crypto",
          "exponent": -8,
          "cmc_id": null,
          "interval": null,
          "min_publishers": 3,
          "min_channel": "real_time",
          "state": "some_future_state",
          "hermes_id": null,
          "quote_currency": "USD"
        }
        "#;

        let symbol: SymbolMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(symbol.state, SymbolState::Unknown);
    }

    #[test]
    fn symbol_metadata_round_trips_bond_fields() {
        let json = r#"
        {
          "pyth_lazer_id": 2,
          "name": "US10Y",
          "symbol": "InterestRate.US10Y",
          "description": "US 10Y",
          "asset_type": "rates",
          "exponent": -8,
          "cmc_id": null,
          "interval": null,
          "min_publishers": 1,
          "min_channel": "real_time",
          "state": "stable",
          "hermes_id": null,
          "quote_currency": null,
          "cusip": "91282CNT4",
          "auction_date": "2026-08-12",
          "issue_date": "2026-08-15",
          "maturity_date": "2036-08-15",
          "coupon": "4.625",
          "next_auction_date": "pending"
        }
        "#;

        let symbol: SymbolMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(symbol.cusip.as_deref(), Some("91282CNT4"));
        assert_eq!(symbol.auction_date.as_deref(), Some("2026-08-12"));
        assert_eq!(symbol.issue_date.as_deref(), Some("2026-08-15"));
        assert_eq!(symbol.maturity_date.as_deref(), Some("2036-08-15"));
        assert_eq!(symbol.coupon.as_deref(), Some("4.625"));
        assert_eq!(symbol.next_auction_date.as_deref(), Some("pending"));

        let out = serde_json::to_value(&symbol).unwrap();
        assert_eq!(out["cusip"], "91282CNT4");
        assert_eq!(out["next_auction_date"], "pending");
    }

    #[test]
    fn symbol_metadata_omits_absent_bond_fields() {
        let json = r#"
        {
          "pyth_lazer_id": 1,
          "name": "BTCUSD",
          "symbol": "Crypto.BTC/USD",
          "description": "BITCOIN / US DOLLAR",
          "asset_type": "crypto",
          "exponent": -8,
          "cmc_id": null,
          "interval": null,
          "min_publishers": 3,
          "min_channel": "real_time",
          "state": "stable",
          "hermes_id": null,
          "quote_currency": "USD"
        }
        "#;

        let symbol: SymbolMetadata = serde_json::from_str(json).unwrap();
        let out = serde_json::to_value(&symbol).unwrap();
        for field in [
            "cusip",
            "auction_date",
            "issue_date",
            "maturity_date",
            "coupon",
            "next_auction_date",
        ] {
            assert!(out.get(field).is_none(), "{field} should be omitted");
        }
    }

    #[test]
    fn test_push_update_price() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 124214124124,

            "update": {
              "type": "price",
              "price": 1234567890,
              "best_bid_price": 1234567891,
              "best_ask_price": 1234567892,
              "trading_status": "halted",
              "market_session": "postMarket"
            }
          },
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::PriceUpdate {
                    price: Some(Price::from_integer(1234567890, 0).unwrap()),
                    best_bid_price: Some(Price::from_integer(1234567891, 0).unwrap()),
                    best_ask_price: Some(Price::from_integer(1234567892, 0).unwrap()),
                    trading_status: Some(TradingStatus::Halted),
                    market_session: Some(MarketSession::PostMarket),
                },
            }),
            id: JrpcId::Int(1),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_push_update_by_identifier() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "identifier": { "scheme": "cusip", "value": { "cusip": "912797NQ4", "quote_currency": "USD" } },
            "source_timestamp": 124214124124,
            "update": { "type": "price", "price": 1234567890, "best_bid_price": null, "best_ask_price": null }
          },
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: None,
                identifier: Some(Identifier::Cusip(Cusip {
                    cusip: "912797NQ4".to_string(),
                    quote_currency: "USD".to_string(),
                })),
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::PriceUpdate {
                    price: Some(Price::from_integer(1234567890, 0).unwrap()),
                    best_bid_price: None,
                    best_ask_price: None,
                    trading_status: None,
                    market_session: None,
                },
            }),
            id: JrpcId::Int(1),
        };

        let parsed = serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap();
        assert_eq!(parsed, expected);
        let reserialized = serde_json::to_value(&parsed).unwrap();
        assert!(reserialized["params"].get("feed_id").is_none());
        assert_eq!(reserialized["params"]["identifier"]["scheme"], "cusip");
        assert_eq!(
            reserialized["params"]["identifier"]["value"]["quote_currency"],
            "USD"
        );
    }

    #[test]
    fn test_unknown_scheme_still_parses() {
        let identifier: Identifier = serde_json::from_str(
            r#"{"scheme": "isin", "value": {"isin": "US91282CRX18", "quote_currency": "USD"}}"#,
        )
        .unwrap();
        assert_eq!(identifier, Identifier::Unknown);
    }

    #[test]
    fn test_feed_update_params_helpers() {
        let update = UpdateParams::price(Some(Price::from_integer(99, 0).unwrap()), None, None);
        let timestamp = TimestampUs::from_micros(1);

        let by_cusip = FeedUpdateParams::by_cusip("912797NQ4", "USD", timestamp, update.clone());
        assert_eq!(by_cusip.feed_id, None);
        assert_eq!(
            by_cusip.identifier,
            Some(Identifier::cusip("912797NQ4", "USD"))
        );
        assert_eq!(
            serde_json::to_value(&by_cusip).unwrap()["identifier"],
            serde_json::json!({"scheme": "cusip", "value": {"cusip": "912797NQ4", "quote_currency": "USD"}})
        );

        let by_id = FeedUpdateParams::by_feed_id(PriceFeedId(1), timestamp, update)
            .with_identifier(Identifier::cusip("912797NQ4", "USD"));
        assert_eq!(by_id.feed_id, Some(PriceFeedId(1)));
        assert_eq!(by_id.identifier, by_cusip.identifier);
    }

    /// A feed_id publisher built with the new crate must emit the exact JSON an older agent
    /// accepts: feed_id as a number and no identifier key.
    #[test]
    fn test_feed_id_params_serialize_as_before() {
        let params = FeedUpdateParams::by_feed_id(
            PriceFeedId(1),
            TimestampUs::from_micros(124214124124),
            UpdateParams::price(
                Some(Price::from_integer(1234567890, 0).unwrap()),
                None,
                None,
            ),
        );
        assert_eq!(
            serde_json::to_value(&params).unwrap(),
            serde_json::json!({
                "feed_id": 1,
                "source_timestamp": 124214124124u64,
                "update": {
                    "type": "price",
                    "price": 1234567890,
                    "best_bid_price": null,
                    "best_ask_price": null,
                    "trading_status": null,
                    "market_session": null
                }
            })
        );
    }

    #[test]
    fn test_push_update_price_string_id() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 124214124124,

            "update": {
              "type": "price",
              "price": 1234567890,
              "best_bid_price": 1234567891,
              "best_ask_price": 1234567892
            }
          },
          "id": "b6bb54a0-ea8d-439d-97a7-3b06befa0e76"
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::PriceUpdate {
                    price: Some(Price::from_integer(1234567890, 0).unwrap()),
                    best_bid_price: Some(Price::from_integer(1234567891, 0).unwrap()),
                    best_ask_price: Some(Price::from_integer(1234567892, 0).unwrap()),
                    trading_status: None,
                    market_session: None,
                },
            }),
            id: JrpcId::String("b6bb54a0-ea8d-439d-97a7-3b06befa0e76".to_string()),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_push_update_price_null_id() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 124214124124,

            "update": {
              "type": "price",
              "price": 1234567890,
              "best_bid_price": 1234567891,
              "best_ask_price": 1234567892
            }
          },
          "id": null
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::PriceUpdate {
                    price: Some(Price::from_integer(1234567890, 0).unwrap()),
                    best_bid_price: Some(Price::from_integer(1234567891, 0).unwrap()),
                    best_ask_price: Some(Price::from_integer(1234567892, 0).unwrap()),
                    trading_status: None,
                    market_session: None,
                },
            }),
            id: JrpcId::Null,
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_push_update_price_without_id() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 745214124124,

            "update": {
              "type": "price",
              "price": 5432,
              "best_bid_price": 5432,
              "best_ask_price": 5432
            }
          }
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(745214124124),
                update: UpdateParams::PriceUpdate {
                    price: Some(Price::from_integer(5432, 0).unwrap()),
                    best_bid_price: Some(Price::from_integer(5432, 0).unwrap()),
                    best_ask_price: Some(Price::from_integer(5432, 0).unwrap()),
                    trading_status: None,
                    market_session: None,
                },
            }),
            id: JrpcId::Null,
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_push_update_price_without_bid_ask() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 124214124124,

            "update": {
              "type": "price",
              "price": 1234567890
            }
          },
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::PriceUpdate {
                    price: Some(Price::from_integer(1234567890, 0).unwrap()),
                    best_bid_price: None,
                    best_ask_price: None,
                    trading_status: None,
                    market_session: None,
                },
            }),
            id: JrpcId::Int(1),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_push_update_funding_rate() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 124214124124,

            "update": {
              "type": "funding_rate",
              "price": 1234567890,
              "rate": 1234567891,
              "funding_rate_interval": "8h"
            }
          },
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::FundingRateUpdate {
                    price: Some(Price::from_integer(1234567890, 0).unwrap()),
                    rate: Rate::from_integer(1234567891, 0).unwrap(),
                    funding_rate_interval: Duration::from_secs(28800).into(),
                },
            }),
            id: JrpcId::Int(1),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }
    #[test]
    fn test_push_update_funding_rate_without_price() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "push_update",
          "params": {
            "feed_id": 1,
            "source_timestamp": 124214124124,

            "update": {
              "type": "funding_rate",
              "rate": 1234567891
            }
          },
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: PushUpdate(FeedUpdateParams {
                feed_id: Some(PriceFeedId(1)),
                identifier: None,
                source_timestamp: TimestampUs::from_micros(124214124124),
                update: UpdateParams::FundingRateUpdate {
                    price: None,
                    rate: Rate::from_integer(1234567891, 0).unwrap(),
                    funding_rate_interval: None,
                },
            }),
            id: JrpcId::Int(1),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_send_get_metadata() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "get_metadata",
          "params": {
            "names": ["BTC/USD"],
            "asset_types": ["crypto"]
          },
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: GetMetadata(GetMetadataParams {
                names: Some(vec!["BTC/USD".to_string()]),
                asset_types: Some(vec!["crypto".to_string()]),
            }),
            id: JrpcId::Int(1),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_get_metadata_without_filters() {
        let json = r#"
        {
          "jsonrpc": "2.0",
          "method": "get_metadata",
          "params": {},
          "id": 1
        }
        "#;

        let expected = PythLazerAgentJrpcV1 {
            jsonrpc: JsonRpcVersion::V2,
            params: GetMetadata(GetMetadataParams {
                names: None,
                asset_types: None,
            }),
            id: JrpcId::Int(1),
        };

        assert_eq!(
            serde_json::from_str::<PythLazerAgentJrpcV1>(json).unwrap(),
            expected
        );
    }

    #[test]
    fn test_response_format_error() {
        let response = serde_json::from_str::<JrpcErrorResponse>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": 2,
              "error": {
                "message": "Internal error",
                "code": -32603
              }
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            response,
            JrpcErrorResponse {
                jsonrpc: JsonRpcVersion::V2,
                error: JrpcErrorObject {
                    code: -32603,
                    message: "Internal error".to_string(),
                    data: None,
                },
                id: JrpcId::Int(2),
            }
        );
    }

    #[test]
    fn test_response_format_error_string_id() {
        let response = serde_json::from_str::<JrpcErrorResponse>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": "62b627dc-5599-43dd-b2c2-9c4d30f4fdb4",
              "error": {
                "message": "Internal error",
                "code": -32603
              }
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            response,
            JrpcErrorResponse {
                jsonrpc: JsonRpcVersion::V2,
                error: JrpcErrorObject {
                    code: -32603,
                    message: "Internal error".to_string(),
                    data: None,
                },
                id: JrpcId::String("62b627dc-5599-43dd-b2c2-9c4d30f4fdb4".to_string())
            }
        );
    }

    #[test]
    pub fn test_response_format_success() {
        let response = serde_json::from_str::<JrpcSuccessResponse<String>>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": 2,
              "result": "success"
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            response,
            JrpcSuccessResponse::<String> {
                jsonrpc: JsonRpcVersion::V2,
                result: "success".to_string(),
                id: JrpcId::Int(2),
            }
        );
    }

    #[test]
    pub fn test_response_format_success_string_id() {
        let response = serde_json::from_str::<JrpcSuccessResponse<String>>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": "62b627dc-5599-43dd-b2c2-9c4d30f4fdb4",
              "result": "success"
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            response,
            JrpcSuccessResponse::<String> {
                jsonrpc: JsonRpcVersion::V2,
                result: "success".to_string(),
                id: JrpcId::String("62b627dc-5599-43dd-b2c2-9c4d30f4fdb4".to_string()),
            }
        );
    }

    #[test]
    pub fn test_parse_response() {
        let success_response = serde_json::from_str::<JrpcResponse<String>>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": 2,
              "result": "success"
            }"#,
        )
        .unwrap();

        assert_eq!(
            success_response,
            JrpcResponse::Success(JrpcSuccessResponse::<String> {
                jsonrpc: JsonRpcVersion::V2,
                result: "success".to_string(),
                id: JrpcId::Int(2),
            })
        );

        let error_response = serde_json::from_str::<JrpcResponse<String>>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": 3,
              "error": {
                "code": -32603,
                "message": "Internal error"
              }
            }"#,
        )
        .unwrap();

        assert_eq!(
            error_response,
            JrpcResponse::Error(JrpcErrorResponse {
                jsonrpc: JsonRpcVersion::V2,
                error: JrpcErrorObject {
                    code: -32603,
                    message: "Internal error".to_string(),
                    data: None,
                },
                id: JrpcId::Int(3),
            })
        );
    }

    #[test]
    pub fn test_parse_response_string_id() {
        let success_response = serde_json::from_str::<JrpcResponse<String>>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": "id-2",
              "result": "success"
            }"#,
        )
        .unwrap();

        assert_eq!(
            success_response,
            JrpcResponse::Success(JrpcSuccessResponse::<String> {
                jsonrpc: JsonRpcVersion::V2,
                result: "success".to_string(),
                id: JrpcId::String("id-2".to_string()),
            })
        );

        let error_response = serde_json::from_str::<JrpcResponse<String>>(
            r#"
            {
              "jsonrpc": "2.0",
              "id": "id-3",
              "error": {
                "code": -32603,
                "message": "Internal error"
              }
            }"#,
        )
        .unwrap();

        assert_eq!(
            error_response,
            JrpcResponse::Error(JrpcErrorResponse {
                jsonrpc: JsonRpcVersion::V2,
                error: JrpcErrorObject {
                    code: -32603,
                    message: "Internal error".to_string(),
                    data: None,
                },
                id: JrpcId::String("id-3".to_string()),
            })
        );
    }
}
