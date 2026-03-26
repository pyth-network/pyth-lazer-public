use crate::config::Config;
use crate::lazer_publisher::LazerPublisher;
use crate::metadata::fetch_metadata;
use crate::websocket_utils::{handle_websocket_error, send_text};
use futures::{AsyncRead, AsyncWrite};
use futures_util::io::{BufReader, BufWriter};
use hyper_util::rt::TokioIo;
use protobuf::{EnumOrUnknown, MessageField};
use pyth_lazer_protocol::PriceFeedId;
use pyth_lazer_protocol::jrpc::{JrpcId, JsonRpcVersion, SymbolMetadata};
use pyth_lazer_publisher_sdk::publisher_update::feed_update::Update;
use pyth_lazer_publisher_sdk::publisher_update::{FeedUpdate, PriceUpdate};
use pyth_lazer_publisher_sdk::state::TradingStatus;
use serde::{Deserialize, Serialize};
use soketto::Sender;
use soketto::handshake::http::Server;
use std::collections::{HashMap, HashSet};
use tokio::time::MissedTickBehavior;
use tokio::{pin, select};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, error, instrument};
use url::Url;

#[derive(Deserialize, Debug)]
struct LegacyJrpcRequest {
    #[allow(dead_code, reason = "validated by serde during deserialization")]
    jsonrpc: JsonRpcVersion,
    #[serde(flatten)]
    method: LegacyMethod,
    #[serde(default)]
    id: JrpcId,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum LegacyMethod {
    GetProductList(
        #[allow(dead_code, reason = "validated by serde during deserialization")]
        Option<EmptyParams>,
    ),
    GetProduct(AccountParams),
    GetAllProducts,
    SubscribePriceSched(AccountParams),
    SubscribePrice(AccountParams),
    UpdatePrice(UpdatePriceParams),
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Deserialize, Debug)]
struct AccountParams {
    account: String,
}

#[derive(Deserialize, Debug)]
struct UpdatePriceParams {
    account: String,
    #[serde(deserialize_with = "serde_this_or_that::as_i64")]
    price: i64,
    #[serde(deserialize_with = "serde_this_or_that::as_u64")]
    conf: u64,
    status: LegacyPriceStatus,
}

#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum LegacyPriceStatus {
    Unknown,
    Trading,
    Halted,
    Auction,
    Ignored,
}

impl LegacyPriceStatus {
    fn to_trading_status(self) -> Option<EnumOrUnknown<TradingStatus>> {
        match self {
            Self::Trading => Some(EnumOrUnknown::new(TradingStatus::TRADING_STATUS_OPEN)),
            Self::Halted => Some(EnumOrUnknown::new(TradingStatus::TRADING_STATUS_HALTED)),
            Self::Unknown | Self::Auction | Self::Ignored => None,
        }
    }
}

#[derive(Serialize)]
struct LegacySuccessResponse<'a, T: Serialize> {
    jsonrpc: &'a str,
    result: T,
    id: &'a JrpcId,
}

#[derive(Serialize)]
struct LegacyErrorResponse<'a> {
    jsonrpc: &'a str,
    error: LegacyErrorObject<'a>,
    id: &'a JrpcId,
}

#[derive(Serialize)]
struct LegacyErrorObject<'a> {
    code: i32,
    message: &'a str,
}

#[derive(Serialize)]
struct LegacyNotification<T: Serialize> {
    jsonrpc: &'static str,
    method: &'static str,
    params: T,
}

#[derive(Serialize)]
struct SubscriptionResult {
    subscription: u64,
}

#[derive(Serialize)]
struct SchedNotificationParams {
    subscription: u64,
}

const JSONRPC_V2: &str = "2.0";
const INTERNAL_ERROR_CODE: i32 = -32603;

fn make_success_value<T: Serialize>(
    id: &JrpcId,
    result: T,
) -> serde_json::Result<serde_json::Value> {
    serde_json::to_value(LegacySuccessResponse {
        jsonrpc: JSONRPC_V2,
        result,
        id,
    })
}

fn make_error_value(id: &JrpcId, message: &str) -> serde_json::Result<serde_json::Value> {
    serde_json::to_value(LegacyErrorResponse {
        jsonrpc: JSONRPC_V2,
        error: LegacyErrorObject {
            code: INTERNAL_ERROR_CODE,
            message,
        },
        id,
    })
}

#[derive(Serialize, Clone, Debug)]
struct ProductAccountDetail {
    account: String,
    attr_dict: HashMap<String, String>,
    price_accounts: Vec<PriceAccountDetail>,
}

/// A complete PriceAccount, including pricing data.
///
/// This is only to support backward type compatibility with the legacy API
/// but the content is not going to be compatible. As in price and conf fields
/// are going to be set to zero.
#[derive(Serialize, Clone, Debug)]
struct PriceAccountDetail {
    account: String,
    price_type: &'static str,
    price_exponent: i16,
    status: &'static str,
    price: i64,
    conf: u64,
    twap: i64,
    twac: i64,
    valid_slot: u64,
    pub_slot: u64,
    prev_slot: u64,
    prev_price: i64,
    prev_conf: u64,
    publisher_accounts: Vec<serde_json::Value>,
}

fn product_detail_from_metadata(sym: &SymbolMetadata) -> ProductAccountDetail {
    let feed_id_str = sym.pyth_lazer_id.0.to_string();

    let mut attr_dict = HashMap::new();
    attr_dict.insert("symbol".to_string(), sym.symbol.clone());
    attr_dict.insert("asset_type".to_string(), sym.asset_type.clone());
    attr_dict.insert("description".to_string(), sym.description.clone());
    if let Some(ref qc) = sym.quote_currency {
        attr_dict.insert("quote_currency".to_string(), qc.clone());
    }

    ProductAccountDetail {
        account: feed_id_str.clone(),
        attr_dict,
        price_accounts: vec![PriceAccountDetail {
            account: feed_id_str,
            price_type: "price",
            price_exponent: sym.exponent,
            status: "trading",
            price: 0,
            conf: 0,
            twap: 0,
            twac: 0,
            valid_slot: 0,
            pub_slot: 0,
            prev_slot: 0,
            prev_price: 0,
            prev_conf: 0,
            publisher_accounts: vec![],
        }],
    }
}

#[instrument(
    skip(server, request, lazer_publisher, config),
    fields(component = "legacy_ws")
)]
pub async fn handle_legacy(
    config: Config,
    server: Server,
    request: hyper::Request<hyper::body::Incoming>,
    lazer_publisher: LazerPublisher,
) {
    if let Err(err) = try_handle_legacy(config, server, request, lazer_publisher).await {
        handle_websocket_error(err);
    }
}

#[instrument(
    skip(server, request, lazer_publisher, config),
    fields(component = "legacy_ws")
)]
async fn try_handle_legacy(
    config: Config,
    server: Server,
    request: hyper::Request<hyper::body::Incoming>,
    lazer_publisher: LazerPublisher,
) -> anyhow::Result<()> {
    let stream = hyper::upgrade::on(request).await?;
    let io = TokioIo::new(stream);
    let stream = BufReader::new(BufWriter::new(io.compat()));
    let (mut ws_sender, mut ws_receiver) = server.into_builder(stream).finish();

    let mut receive_buf = Vec::new();
    let mut next_subscription_id: u64 = 1;
    let mut sched_subscriptions: HashSet<u64> = HashSet::new();

    let mut sched_interval = tokio::time::interval(config.legacy_sched_interval_duration);
    sched_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        receive_buf.clear();
        {
            let receive = async { ws_receiver.receive(&mut receive_buf).await };
            pin!(receive);
            loop {
                select! {
                    result = &mut receive => {
                        result?;
                        break;
                    }
                    _ = sched_interval.tick() => {
                        send_sched_notifications(&mut ws_sender, &sched_subscriptions).await;
                    }
                }
            }
        }

        let request_text = match std::str::from_utf8(&receive_buf) {
            Ok(s) => s.to_string(),
            Err(_) => {
                debug!("received non-utf8 data, ignoring");
                continue;
            }
        };

        let parsed: serde_json::Value = match serde_json::from_str(&request_text) {
            Ok(v) => v,
            Err(err) => {
                let id = JrpcId::Int(0);
                let response = make_error_value(&id, &err.to_string())?;
                send_text(&mut ws_sender, &serde_json::to_string(&response)?).await?;
                continue;
            }
        };

        match parsed {
            serde_json::Value::Array(items) => {
                let mut responses = Vec::with_capacity(items.len());
                for raw in items {
                    responses.push(
                        dispatch_request(
                            &raw,
                            &lazer_publisher,
                            &config.history_service_url,
                            &mut next_subscription_id,
                            &mut sched_subscriptions,
                        )
                        .await?,
                    );
                }
                send_text(&mut ws_sender, &serde_json::to_string(&responses)?).await?;
            }
            raw @ serde_json::Value::Object(_) => {
                let response = dispatch_request(
                    &raw,
                    &lazer_publisher,
                    &config.history_service_url,
                    &mut next_subscription_id,
                    &mut sched_subscriptions,
                )
                .await?;
                send_text(&mut ws_sender, &serde_json::to_string(&response)?).await?;
            }
            _ => {
                let id = JrpcId::Int(0);
                let response = make_error_value(&id, "expected JSON object or array")?;
                send_text(&mut ws_sender, &serde_json::to_string(&response)?).await?;
            }
        }
    }
}

async fn send_sched_notifications<T: AsyncRead + AsyncWrite + Unpin>(
    sender: &mut Sender<T>,
    sched_subscriptions: &HashSet<u64>,
) {
    for &sub_id in sched_subscriptions {
        let notification = LegacyNotification {
            jsonrpc: JSONRPC_V2,
            method: "notify_price_sched",
            params: SchedNotificationParams {
                subscription: sub_id,
            },
        };
        if let Ok(json) = serde_json::to_string(&notification) {
            if let Err(err) = send_text(sender, &json).await {
                debug!("failed to send notify_price_sched: {err}");
                return;
            }
        }
    }
}

async fn dispatch_request(
    raw: &serde_json::Value,
    lazer_publisher: &LazerPublisher,
    metadata_url: &Url,
    next_subscription_id: &mut u64,
    sched_subscriptions: &mut HashSet<u64>,
) -> serde_json::Result<serde_json::Value> {
    let fallback_id = JrpcId::Int(0);

    let request: LegacyJrpcRequest = match serde_json::from_value(raw.clone()) {
        Ok(r) => r,
        Err(err) => {
            return make_error_value(&fallback_id, &err.to_string());
        }
    };

    let id = &request.id;
    match request.method {
        LegacyMethod::GetProductList(_) => handle_get_product_list(metadata_url, id).await,
        LegacyMethod::GetProduct(params) => {
            handle_get_product(metadata_url, &params.account, id).await
        }
        LegacyMethod::GetAllProducts => handle_get_all_products(metadata_url, id).await,
        LegacyMethod::SubscribePriceSched(_params) => {
            handle_subscribe_price_sched(id, next_subscription_id, sched_subscriptions).await
        }
        LegacyMethod::SubscribePrice(_params) => handle_subscribe_price(id).await,
        LegacyMethod::UpdatePrice(params) => handle_update_price(params, id, lazer_publisher).await,
    }
}

async fn handle_get_product_list(
    metadata_url: &Url,
    id: &JrpcId,
) -> serde_json::Result<serde_json::Value> {
    match fetch_metadata(metadata_url).await {
        Ok(metadata) => {
            let products: Vec<_> = metadata.iter().map(product_detail_from_metadata).collect();
            make_success_value(id, &products)
        }
        Err(err) => {
            error!("error while retrieving metadata: {err:?}");
            make_error_value(id, &err.to_string())
        }
    }
}

async fn handle_get_product(
    metadata_url: &Url,
    account: &str,
    id: &JrpcId,
) -> serde_json::Result<serde_json::Value> {
    match fetch_metadata(metadata_url).await {
        Ok(metadata) => {
            let detail = metadata
                .iter()
                .map(product_detail_from_metadata)
                .find(|d| d.account == account);
            match detail {
                Some(d) => make_success_value(id, &d),
                None => make_error_value(id, "product account not found"),
            }
        }
        Err(err) => {
            error!("error while retrieving metadata: {err:?}");
            make_error_value(id, &err.to_string())
        }
    }
}

async fn handle_get_all_products(
    metadata_url: &Url,
    id: &JrpcId,
) -> serde_json::Result<serde_json::Value> {
    match fetch_metadata(metadata_url).await {
        Ok(metadata) => {
            let products: Vec<_> = metadata.iter().map(product_detail_from_metadata).collect();
            make_success_value(id, &products)
        }
        Err(err) => {
            error!("error while retrieving metadata: {err:?}");
            make_error_value(id, &err.to_string())
        }
    }
}

async fn handle_subscribe_price_sched(
    id: &JrpcId,
    next_subscription_id: &mut u64,
    sched_subscriptions: &mut HashSet<u64>,
) -> serde_json::Result<serde_json::Value> {
    let sub_id = *next_subscription_id;
    *next_subscription_id += 1;
    sched_subscriptions.insert(sub_id);
    make_success_value(
        id,
        SubscriptionResult {
            subscription: sub_id,
        },
    )
}

async fn handle_subscribe_price(id: &JrpcId) -> serde_json::Result<serde_json::Value> {
    make_error_value(id, "this method is not supported in the legacy adapter")
}

async fn handle_update_price(
    params: UpdatePriceParams,
    id: &JrpcId,
    lazer_publisher: &LazerPublisher,
) -> serde_json::Result<serde_json::Value> {
    let feed_id = match params.account.parse::<u32>().ok().map(PriceFeedId) {
        Some(fid) => fid,
        None => return make_error_value(id, "invalid price account"),
    };

    let trading_status = params.status.to_trading_status();

    let conf_i64 = match i64::try_from(params.conf) {
        Ok(conf_i64) => conf_i64,
        Err(_) => i64::MAX,
    };

    let feed_update = FeedUpdate {
        feed_id: Some(feed_id.0),
        source_timestamp: MessageField::some(
            protobuf::well_known_types::timestamp::Timestamp::now(),
        ),
        update: Some(Update::PriceUpdate(PriceUpdate {
            price: Some(params.price),
            best_bid_price: Some(params.price.saturating_sub(conf_i64)),
            best_ask_price: Some(params.price.saturating_add(conf_i64)),
            trading_status,
            market_session: None,
            special_fields: Default::default(),
        })),
        special_fields: Default::default(),
    };

    match lazer_publisher.push_feed_update(feed_update).await {
        Ok(()) => make_success_value(id, 0),
        Err(err) => {
            error!("error while sending update: {err:?}");
            make_error_value(id, &err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_update_price_with_numbers() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "update_price",
            "params": {
                "account": "abc123",
                "price": 42002,
                "conf": 3,
                "status": "trading"
            },
            "id": 1
        }"#;
        let req: LegacyJrpcRequest = serde_json::from_str(json).unwrap();
        match req.method {
            LegacyMethod::UpdatePrice(params) => {
                assert_eq!(params.account, "abc123");
                assert_eq!(params.price, 42002);
                assert_eq!(params.conf, 3);
                assert!(matches!(params.status, LegacyPriceStatus::Trading));
            }
            other => panic!("expected UpdatePrice, got: {other:?}"),
        }
    }

    #[test]
    fn test_deserialize_update_price_with_string_numbers() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "update_price",
            "params": {
                "account": "abc123",
                "price": "42002",
                "conf": "3",
                "status": "halted"
            },
            "id": 1
        }"#;
        let req: LegacyJrpcRequest = serde_json::from_str(json).unwrap();
        match req.method {
            LegacyMethod::UpdatePrice(params) => {
                assert_eq!(params.price, 42002);
                assert_eq!(params.conf, 3);
                assert!(matches!(params.status, LegacyPriceStatus::Halted));
            }
            other => panic!("expected UpdatePrice, got: {other:?}"),
        }
    }

    #[test]
    fn test_deserialize_get_product_list() {
        let json = r#"{"jsonrpc": "2.0", "method": "get_product_list", "id": 1}"#;
        let req: LegacyJrpcRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req.method, LegacyMethod::GetProductList(_)));
        assert_eq!(req.id, JrpcId::Int(1));
    }

    #[test]
    fn test_deserialize_get_product_list_with_empty_params() {
        let json = r#"{"jsonrpc": "2.0", "method": "get_product_list", "params": {}, "id": 1}"#;
        let req: LegacyJrpcRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req.method, LegacyMethod::GetProductList(_)));
        assert_eq!(req.id, JrpcId::Int(1));
    }

    #[test]
    fn test_deserialize_subscribe_price_sched() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "subscribe_price_sched",
            "params": {"account": "some_key"},
            "id": 5
        }"#;
        let req: LegacyJrpcRequest = serde_json::from_str(json).unwrap();
        match req.method {
            LegacyMethod::SubscribePriceSched(params) => {
                assert_eq!(params.account, "some_key");
            }
            other => panic!("expected SubscribePriceSched, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_batch_request() {
        let json = r#"[
            {"jsonrpc": "2.0", "method": "get_product_list", "id": 1},
            {"jsonrpc": "2.0", "method": "get_all_products", "id": 2}
        ]"#;
        let requests: Vec<LegacyJrpcRequest> = serde_json::from_str(json).unwrap();
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            requests[0].method,
            LegacyMethod::GetProductList(_)
        ));
        assert!(matches!(requests[1].method, LegacyMethod::GetAllProducts));
    }

    #[test]
    fn test_error_response_format() {
        let id = JrpcId::Int(0);
        let err = make_error_value(&id, "product account not found").unwrap();
        assert_eq!(err["jsonrpc"], "2.0");
        assert_eq!(err["error"]["code"], -32603);
        assert_eq!(err["error"]["message"], "product account not found");
        assert_eq!(err["id"], 0);
    }

    #[test]
    fn test_success_response_format() {
        let id = JrpcId::Int(7);
        let resp = make_success_value(&id, SubscriptionResult { subscription: 42 }).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["result"]["subscription"], 42);
        assert_eq!(resp["id"], 7);
    }

    #[test]
    fn test_product_detail_from_metadata() {
        use pyth_lazer_protocol::SymbolState;
        use pyth_lazer_protocol::api::Channel;
        use pyth_lazer_protocol::time::FixedRate;

        let sym = SymbolMetadata {
            pyth_lazer_id: PriceFeedId(1),
            name: "BTC".to_string(),
            symbol: "Crypto.BTC/USD".to_string(),
            description: "BTC/USD".to_string(),
            asset_type: "Crypto".to_string(),
            exponent: -8,
            cmc_id: None,
            funding_rate_interval: None,
            min_publishers: 1,
            min_channel: Channel::FixedRate(FixedRate::MIN),
            state: SymbolState::Stable,
            hermes_id: None,
            quote_currency: Some("USD".to_string()),
            nasdaq_symbol: None,
        };

        let detail = product_detail_from_metadata(&sym);

        assert_eq!(detail.account, "1");
        assert_eq!(detail.attr_dict.len(), 4);
        assert_eq!(detail.attr_dict["symbol"], "Crypto.BTC/USD");
        assert_eq!(detail.attr_dict["asset_type"], "Crypto");
        assert_eq!(detail.attr_dict["description"], "BTC/USD");
        assert_eq!(detail.attr_dict["quote_currency"], "USD");

        assert_eq!(detail.price_accounts.len(), 1);
        let pa = &detail.price_accounts[0];
        assert_eq!(pa.account, "1");
        assert_eq!(pa.price_exponent, -8);
        assert_eq!(pa.price_type, "price");
        assert_eq!(pa.status, "trading");
        assert_eq!(pa.price, 0);
        assert_eq!(pa.conf, 0);
        assert_eq!(pa.twap, 0);
        assert_eq!(pa.twac, 0);
        assert_eq!(pa.valid_slot, 0);
        assert_eq!(pa.pub_slot, 0);
        assert_eq!(pa.prev_slot, 0);
        assert_eq!(pa.prev_price, 0);
        assert_eq!(pa.prev_conf, 0);
        assert!(pa.publisher_accounts.is_empty());
    }

    #[test]
    fn test_product_detail_without_quote_currency() {
        use pyth_lazer_protocol::SymbolState;
        use pyth_lazer_protocol::api::Channel;
        use pyth_lazer_protocol::time::FixedRate;

        let sym = SymbolMetadata {
            pyth_lazer_id: PriceFeedId(42),
            name: "AAPL".to_string(),
            symbol: "Equity.AAPL/USD".to_string(),
            description: "AAPL/USD".to_string(),
            asset_type: "Equity".to_string(),
            exponent: -4,
            cmc_id: None,
            funding_rate_interval: None,
            min_publishers: 1,
            min_channel: Channel::FixedRate(FixedRate::MIN),
            state: SymbolState::Stable,
            hermes_id: None,
            quote_currency: None,
            nasdaq_symbol: None,
        };

        let detail = product_detail_from_metadata(&sym);

        assert_eq!(detail.attr_dict.len(), 3);
        assert_eq!(detail.attr_dict["symbol"], "Equity.AAPL/USD");
        assert_eq!(detail.attr_dict["asset_type"], "Equity");
        assert_eq!(detail.attr_dict["description"], "AAPL/USD");
        assert!(!detail.attr_dict.contains_key("quote_currency"));
        assert_eq!(detail.price_accounts[0].price_exponent, -4);
    }

    #[test]
    fn test_legacy_price_status_to_trading_status() {
        assert_eq!(
            LegacyPriceStatus::Trading
                .to_trading_status()
                .unwrap()
                .enum_value()
                .unwrap(),
            TradingStatus::TRADING_STATUS_OPEN
        );
        assert_eq!(
            LegacyPriceStatus::Halted
                .to_trading_status()
                .unwrap()
                .enum_value()
                .unwrap(),
            TradingStatus::TRADING_STATUS_HALTED
        );
        assert!(LegacyPriceStatus::Unknown.to_trading_status().is_none());
        assert!(LegacyPriceStatus::Ignored.to_trading_status().is_none());
        assert!(LegacyPriceStatus::Auction.to_trading_status().is_none());
    }
}
