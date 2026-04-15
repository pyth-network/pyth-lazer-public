use {
    crate::api::{Channel, MerklePriceFeedId},
    anyhow::{anyhow, bail, Result},
    serde::{Deserialize, Serialize},
    serde_with::{serde_as, DisplayFromStr},
    std::fmt,
};

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "utoipa",
    schema(example = "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43")
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
/// A price id is a 32-byte hex string, optionally prefixed with "0x".
/// Price ids are case insensitive.
///
/// Examples:
/// * 0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43
/// * e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43
///
/// See https://pyth.network/developers/price-feed-ids for a list of all price feed ids.
pub struct PriceIdInput(pub String);

impl PriceIdInput {
    fn is_valid(&self) -> bool {
        let normalized = self.normalize();
        normalized.0.len() == 64 && normalized.0.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    fn normalize(&self) -> Self {
        let normalized = self
            .0
            .strip_prefix("0x")
            .or_else(|| self.0.strip_prefix("0X"))
            .unwrap_or(&self.0);
        Self(normalized.to_string())
    }

    pub fn parse(&self) -> Result<MerklePriceFeedId> {
        if !self.is_valid() {
            bail!("Invalid price id: {}", self.0);
        }
        let normalized = self.normalize();
        let bytes = hex::decode(normalized.0)
            .map_err(|e| anyhow!("Failed to decode price id: {}, error: {}", self.0, e))?;
        bytes
            .try_into()
            .map_err(|_| anyhow!("Invalid price length: {}", self.0))
    }
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetType {
    Crypto,
    Fx,
    Equity,
    Metal,
    Rates,
    CryptoRedemptionRate,
    Commodities,
    CryptoIndex,
    CryptoNav,
    Eco,
    Kalshi,
}

impl fmt::Display for AssetType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AssetType::Crypto => write!(f, "crypto"),
            AssetType::Fx => write!(f, "fx"),
            AssetType::Equity => write!(f, "equity"),
            AssetType::Metal => write!(f, "metal"),
            AssetType::Rates => write!(f, "rates"),
            AssetType::CryptoRedemptionRate => write!(f, "crypto_redemption_rate"),
            AssetType::Commodities => write!(f, "commodities"),
            AssetType::CryptoIndex => write!(f, "crypto_index"),
            AssetType::CryptoNav => write!(f, "crypto_nav"),
            AssetType::Eco => write!(f, "eco"),
            AssetType::Kalshi => write!(f, "kalshi"),
        }
    }
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "utoipa",
    schema(example = "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43")
)]
#[derive(Debug, Clone, Hash, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct RpcPriceIdentifier(pub String);

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct ParsedPriceUpdate {
    pub ema_price: RpcPrice,
    pub id: RpcPriceIdentifier,
    pub metadata: RpcPriceFeedMetadataV2,
    pub price: RpcPrice,
}

#[serde_as]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
/// A price with a degree of uncertainty at a certain time, represented as a price +- a confidence
/// interval.
///
/// The confidence interval roughly corresponds to the standard error of a normal distribution.
/// Both the price and confidence are stored in a fixed-point numeric representation, `x *
/// 10^expo`, where `expo` is the exponent. For example:
pub struct RpcPrice {
    /// The confidence interval associated with the price, stored as a string to avoid precision loss
    #[cfg_attr(feature = "utoipa", schema(value_type = String, example = "509500001"))]
    #[serde_as(as = "DisplayFromStr")]
    pub conf: u64,
    /// The exponent associated with both the price and confidence interval. Multiply those values
    /// by `10^expo` to get the real value.
    #[cfg_attr(feature = "utoipa", schema(example = -8))]
    pub expo: i32,
    /// The price itself, stored as a string to avoid precision loss
    #[cfg_attr(feature = "utoipa", schema(value_type = String, example = "2920679499999"))]
    #[serde_as(as = "DisplayFromStr")]
    pub price: i64,
    /// When the price was published. The `publish_time` is a unix timestamp, i.e., the number of
    /// seconds since the Unix epoch (00:00:00 UTC on 1 Jan 1970).
    #[cfg_attr(feature = "utoipa", schema(example = 1717632000))]
    pub publish_time: i64,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct RpcPriceFeedMetadataV2 {
    #[cfg_attr(feature = "utoipa", schema(example = 1717632000))]
    pub prev_publish_time: i64,
    #[cfg_attr(feature = "utoipa", schema(example = 1717632000))]
    pub proof_available_time: i64,
    #[cfg_attr(feature = "utoipa", schema(minimum = 0, example = 85480034))]
    pub slot: i64,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceFeedAttributes {
    pub asset_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cms_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cqs_symbol: Option<String>,
    pub description: String,
    pub display_symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generic_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nasdaq_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_interval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    pub symbol: String,
    pub min_channel: Channel,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceFeedMetadata {
    pub id: RpcPriceIdentifier,
    pub attributes: PriceFeedAttributes,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct WsPriceFeed {
    pub id: RpcPriceIdentifier,
    pub price: RpcPrice,
    pub ema_price: RpcPrice,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<WsPriceFeedMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vaa: Option<String>,
}

#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct WsPriceFeedMetadata {
    pub slot: u64,
    pub emitter_chain: u16,
    pub price_service_receive_time: i64,
    pub prev_publish_time: i64,
}

/// Incoming JSON on Hermes WebSocket `/ws` (legacy Hermes wire format; flat `type` + fields).
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HermesWsClientMessage {
    #[serde(rename = "subscribe")]
    Subscribe {
        ids: Vec<PriceIdInput>,
        #[serde(default)]
        verbose: bool,
        #[serde(default)]
        binary: bool,
        #[serde(default)]
        #[allow(dead_code)]
        // reason = Kept for backward compatibility. Lazer merkle updates cannot be out-of-order.
        allow_out_of_order: bool,
        #[serde(default)]
        ignore_invalid_price_ids: bool,
    },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { ids: Vec<PriceIdInput> },
}

/// Outgoing JSON on Hermes WebSocket `/ws` (legacy Hermes wire format).
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
// PriceUpdate is the most frequent message
pub enum HermesWsServerMessage {
    #[serde(rename = "response")]
    Response(HermesWsServerResponse),
    #[serde(rename = "price_update")]
    PriceUpdate { price_feed: WsPriceFeed },
}

/// Body of a `{"type":"response",...}` message (`status` + optional `error`).
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum HermesWsServerResponse {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Err { error: String },
}

#[cfg(test)]
mod tests {
    use super::PriceIdInput;

    #[test]
    fn validates_price_id_with_and_without_prefix() {
        let valid = &PriceIdInput(
            "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43".to_string(),
        );
        assert!(valid.is_valid());

        let valid = &PriceIdInput(
            "0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43".to_string(),
        );
        assert!(valid.is_valid());

        let valid = &PriceIdInput(
            "0Xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43".to_string(),
        );
        assert!(valid.is_valid());
    }

    #[test]
    fn rejects_invalid_price_id() {
        let invalid = &PriceIdInput("abc123".to_string());
        assert!(!invalid.is_valid());

        let invalid = &PriceIdInput(
            "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b4z".to_string(),
        );
        assert!(!invalid.is_valid());
    }
}
