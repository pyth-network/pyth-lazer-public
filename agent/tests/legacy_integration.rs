#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use protobuf::Message;
use pyth_lazer_agent::Config;
use pyth_lazer_protocol::api::Channel;
use pyth_lazer_protocol::jrpc::SymbolMetadata;
use pyth_lazer_protocol::time::FixedRate;
use pyth_lazer_protocol::{PriceFeedId, SymbolState};
use pyth_lazer_publisher_sdk::transaction::lazer_transaction::Payload;
use pyth_lazer_publisher_sdk::transaction::{LazerTransaction, SignedLazerTransaction};
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use url::Url;
use {
    futures_util::{SinkExt, StreamExt},
    tracing::{Instrument, info_span},
};

fn get_private_key_file() -> NamedTempFile {
    let private_key_string = "[105,175,146,91,32,145,164,199,37,111,139,255,44,225,5,247,154,170,238,70,47,15,9,48,102,87,180,50,50,38,148,243,62,148,219,72,222,170,8,246,176,33,205,29,118,11,220,163,214,204,46,49,132,94,170,173,244,39,179,211,177,70,252,31]";
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file
        .as_file_mut()
        .write_all(private_key_string.as_bytes())
        .unwrap();
    temp_file.flush().unwrap();
    temp_file
}

fn test_metadata_json() -> String {
    let metadata = vec![
        SymbolMetadata {
            pyth_lazer_id: PriceFeedId(42),
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
            cusip: None,
            auction_date: None,
            issue_date: None,
            maturity_date: None,
            coupon: None,
            next_auction_date: None,
        },
        SymbolMetadata {
            pyth_lazer_id: PriceFeedId(1527),
            name: "US10Y".to_string(),
            symbol: "InterestRate.US10Y".to_string(),
            description: "US 10Y".to_string(),
            asset_type: "Rates".to_string(),
            exponent: -8,
            cmc_id: None,
            funding_rate_interval: None,
            min_publishers: 1,
            min_channel: Channel::FixedRate(FixedRate::MIN),
            state: SymbolState::Stable,
            hermes_id: None,
            quote_currency: Some("%".to_string()),
            nasdaq_symbol: None,
            cusip: Some("91282CRF0".to_string()),
            auction_date: Some("2026-08-12".to_string()),
            issue_date: Some("2026-08-17".to_string()),
            maturity_date: Some("2036-08-15".to_string()),
            coupon: Some("4.625".to_string()),
            next_auction_date: Some("pending".to_string()),
        },
        SymbolMetadata {
            pyth_lazer_id: PriceFeedId(2903),
            name: "US10Y/USD".to_string(),
            symbol: "InterestRate.US10Y/USD".to_string(),
            description: "US 10Y price".to_string(),
            asset_type: "Rates".to_string(),
            exponent: -8,
            cmc_id: None,
            funding_rate_interval: None,
            min_publishers: 1,
            min_channel: Channel::FixedRate(FixedRate::MIN),
            state: SymbolState::Stable,
            hermes_id: None,
            quote_currency: Some("USD".to_string()),
            nasdaq_symbol: None,
            cusip: Some("91282CRF0".to_string()),
            auction_date: None,
            issue_date: None,
            maturity_date: None,
            coupon: None,
            next_auction_date: None,
        },
    ];
    serde_json::to_string(&metadata).unwrap()
}

async fn start_mock_history_service() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let metadata_json = test_metadata_json();

    #[allow(clippy::disallowed_methods, reason = "instrumented")]
    let handle = tokio::spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let json = metadata_json.clone();
                tokio::spawn(
                    async move {
                        let _ = http1::Builder::new()
                            .serve_connection(
                                TokioIo::new(stream),
                                service_fn(move |_req: hyper::Request<Incoming>| {
                                    let json = json.clone();
                                    async move {
                                        Ok::<_, hyper::Error>(
                                            Response::builder()
                                                .status(StatusCode::OK)
                                                .header("content-type", "application/json")
                                                .body(http_body_util::Full::new(
                                                    hyper::body::Bytes::from(json),
                                                ))
                                                .unwrap(),
                                        )
                                    }
                                }),
                            )
                            .await;
                    }
                    .instrument(info_span!("mock history service connection handler task")),
                );
            }
        }
        .instrument(info_span!("mock history service task")),
    );

    (port, handle)
}

async fn start_agent(legacy_sched_interval_duration: Duration) -> (u16, NamedTempFile) {
    start_agent_with_relayers(legacy_sched_interval_duration, vec![]).await
}

/// Accepts agent connections and yields the feed ids of every publisher update received.
async fn start_mock_relayer() -> (Url, tokio::sync::mpsc::Receiver<Vec<u32>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!(
        "ws://{}/v1/transaction",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(64);
    #[allow(clippy::disallowed_methods, reason = "instrumented")]
    tokio::spawn(
        async move {
            while let Ok((stream, _)) = listener.accept().await {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    continue;
                };
                let (_writer, mut reader) = ws.split();
                while let Some(Ok(msg)) = reader.next().await {
                    let TungsteniteMessage::Binary(bytes) = msg else {
                        continue;
                    };
                    let signed = SignedLazerTransaction::parse_from_bytes(&bytes).unwrap();
                    let transaction =
                        LazerTransaction::parse_from_bytes(signed.payload.as_deref().unwrap())
                            .unwrap();
                    let Some(Payload::PublisherUpdate(update)) = transaction.payload else {
                        continue;
                    };
                    let feed_ids = update.updates.iter().map(|u| u.feed_id.unwrap()).collect();
                    let _ = sender.send(feed_ids).await;
                }
            }
        }
        .instrument(info_span!("mock relayer")),
    );
    (url, receiver)
}

async fn start_agent_with_relayers(
    legacy_sched_interval_duration: Duration,
    relayer_urls: Vec<Url>,
) -> (u16, NamedTempFile) {
    let (history_port, _history_handle) = start_mock_history_service().await;
    let history_url = Url::from_str(&format!("http://127.0.0.1:{history_port}")).unwrap();

    let signing_key_file = get_private_key_file();

    let agent_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent_port = agent_listener.local_addr().unwrap().port();
    drop(agent_listener);

    let config = Config {
        listen_address: format!("127.0.0.1:{agent_port}").parse().unwrap(),
        relayer_urls,
        relayer_connections: None,
        authorization_token: None,
        publish_keypair_path: PathBuf::from(signing_key_file.path()),
        publish_interval_duration: Duration::from_millis(25),
        history_service_url: history_url,
        enable_update_deduplication: false,
        update_deduplication_ttl: Default::default(),
        proxy_url: None,
        legacy_sched_interval_duration,
    };

    #[allow(clippy::disallowed_methods, reason = "instrumented")]
    tokio::spawn(pyth_lazer_agent::run(config).instrument(info_span!("lazer publisher task")));

    tokio::time::sleep(Duration::from_millis(100)).await;

    (agent_port, signing_key_file)
}

async fn send_jrpc(
    ws: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        TungsteniteMessage,
    >,
    value: &serde_json::Value,
) {
    ws.send(TungsteniteMessage::Text(
        serde_json::to_string(value).unwrap().into(),
    ))
    .await
    .unwrap();
}

async fn recv_json(
    ws: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> serde_json::Value {
    let msg = tokio::time::timeout(Duration::from_secs(20), ws.next())
        .await
        .expect("timeout waiting for WS message")
        .expect("stream ended")
        .expect("WS error");
    match msg {
        TungsteniteMessage::Text(text) => {
            serde_json::from_str(&text).expect("invalid JSON from server")
        }
        other => panic!("expected text message, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_legacy_user_journey() {
    let (agent_port, _signing_key_file) = start_agent(Duration::from_millis(200)).await;

    let url = format!("ws://127.0.0.1:{agent_port}/v1/legacy");
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // get_product_list
    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({"jsonrpc": "2.0", "method": "get_product_list", "id": 1}),
    )
    .await;

    let resp = recv_json(&mut ws_stream).await;
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    let products = resp["result"].as_array().expect("result should be array");
    assert_eq!(products.len(), 3);
    let product = &products[0];
    assert_eq!(product["attr_dict"]["symbol"], "Crypto.BTC/USD");
    let price = product["price"].as_array().unwrap();
    assert_eq!(price.len(), 1);
    assert_eq!(price[0]["price_exponent"], -8);
    let price_account = price[0]["account"].as_str().unwrap().to_string();

    // subscribe_price_sched
    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "subscribe_price_sched",
            "params": {"account": price_account},
            "id": 2
        }),
    )
    .await;

    let resp = recv_json(&mut ws_stream).await;
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 2);
    let subscription_id = resp["result"]["subscription"]
        .as_u64()
        .expect("subscription should be a number");
    assert_eq!(subscription_id, 1);

    // Wait for two notify_price_sched and verify the interval between them
    let first_notification = recv_json(&mut ws_stream).await;
    let t1 = tokio::time::Instant::now();
    assert_eq!(first_notification["jsonrpc"], "2.0");
    assert_eq!(first_notification["method"], "notify_price_sched");
    assert_eq!(
        first_notification["params"]["subscription"],
        subscription_id
    );
    assert!(first_notification.get("id").is_none());

    let second_notification = recv_json(&mut ws_stream).await;
    let elapsed = t1.elapsed();
    assert_eq!(second_notification["jsonrpc"], "2.0");
    assert_eq!(second_notification["method"], "notify_price_sched");
    assert_eq!(
        second_notification["params"]["subscription"],
        subscription_id
    );
    assert!(second_notification.get("id").is_none());

    let expected = Duration::from_millis(200);
    let tolerance = Duration::from_millis(100);
    assert!(
        elapsed > expected.saturating_sub(tolerance) && elapsed < expected + tolerance,
        "interval between notifications was {elapsed:?}, expected ~{expected:?}"
    );

    // update_price
    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "update_price",
            "params": {
                "account": price_account,
                "price": 4426101900000_i64,
                "conf": 4271150000_u64,
                "status": "trading"
            },
            "id": 3
        }),
    )
    .await;

    let resp = recv_json(&mut ws_stream).await;
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["result"], 0);

    ws_sink.close().await.unwrap();
}

/// Verify that a successful update_price resets the sched timer so the next
/// notification arrives a full interval after the update, not sooner.
#[tokio::test]
async fn test_update_price_resets_sched_timer() {
    let sched_interval = Duration::from_millis(300);
    let (agent_port, _signing_key_file) = start_agent(sched_interval).await;

    let url = format!("ws://127.0.0.1:{agent_port}/v1/legacy");
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Subscribe
    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "subscribe_price_sched",
            "params": {"account": "42"},
            "id": 1
        }),
    )
    .await;
    let resp = recv_json(&mut ws_stream).await;
    assert_eq!(resp["result"]["subscription"], 1);

    // Wait for first notification to establish baseline
    let first = recv_json(&mut ws_stream).await;
    assert_eq!(first["method"], "notify_price_sched");

    // Immediately after the notification, wait ~half the interval then send
    // update_price, which should reset the timer.
    tokio::time::sleep(sched_interval / 2).await;

    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "update_price",
            "params": {
                "account": "42",
                "price": 100,
                "conf": 1,
                "status": "trading"
            },
            "id": 2
        }),
    )
    .await;

    let resp = recv_json(&mut ws_stream).await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"], 0);

    // Now measure how long until the next notification. If the timer was
    // reset by update_price, it should be close to sched_interval from now
    // (not sched_interval/2 which would be the remainder without reset).
    let t_after_update = tokio::time::Instant::now();
    let next_notification = recv_json(&mut ws_stream).await;
    let elapsed = t_after_update.elapsed();
    assert_eq!(next_notification["method"], "notify_price_sched");

    // The notification should arrive ~sched_interval after the update,
    // not ~sched_interval/2 (the leftover from the old timer).
    let min_expected = sched_interval.mul_f64(0.6);
    assert!(
        elapsed >= min_expected,
        "notification arrived too soon ({elapsed:?}), expected at least {min_expected:?} — \
         timer was not properly reset by update_price"
    );

    ws_sink.close().await.unwrap();
}

#[tokio::test]
async fn test_bond_metadata_passthrough() {
    let (agent_port, _signing_key_file) = start_agent(Duration::from_millis(200)).await;
    let expected = serde_json::json!({
        "cusip": "91282CRF0",
        "auction_date": "2026-08-12",
        "issue_date": "2026-08-17",
        "maturity_date": "2036-08-15",
        "coupon": "4.625",
        "next_auction_date": "pending",
    });

    let (ws_stream, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{agent_port}/v1/jrpc"))
            .await
            .unwrap();
    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({"jsonrpc": "2.0", "method": "get_metadata", "params": {}, "id": 1}),
    )
    .await;
    let resp = recv_json(&mut ws_stream).await;
    let bond = &resp["result"][1];
    assert_eq!(bond["symbol"], "InterestRate.US10Y");
    for (key, value) in expected.as_object().unwrap() {
        assert_eq!(&bond[key], value, "get_metadata {key}");
    }

    let (ws_stream, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{agent_port}/v1/legacy"))
            .await
            .unwrap();
    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    send_jrpc(
        &mut ws_sink,
        &serde_json::json!({"jsonrpc": "2.0", "method": "get_product_list", "id": 1}),
    )
    .await;
    let resp = recv_json(&mut ws_stream).await;
    let attr_dict = &resp["result"][1]["attr_dict"];
    assert_eq!(attr_dict["symbol"], "InterestRate.US10Y");
    for (key, value) in expected.as_object().unwrap() {
        assert_eq!(&attr_dict[key], value, "get_product_list {key}");
    }
}

/// Publishing by identifier (CUSIP plus quote currency) reaches the relayer with the feed id the
/// metadata maps to; the pre-existing feed_id shape still works; unknown identifiers are dropped;
/// an identifier without a quote currency and a request with neither field are errors.
#[tokio::test]
async fn test_push_update_by_cusip_routes_to_feed() {
    let (relayer_url, mut received) = start_mock_relayer().await;
    let (agent_port, _signing_key_file) =
        start_agent_with_relayers(Duration::from_millis(200), vec![relayer_url]).await;

    let (ws_stream, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{agent_port}/v1/jrpc"))
            .await
            .unwrap();
    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    let update = serde_json::json!({"type": "price", "price": 9980000000i64});

    // Resend the yield update until the agent has loaded its map and routed it to feed 1527;
    // before the first load the agent answers with an error.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        send_jrpc(
            &mut ws_sink,
            &serde_json::json!({"jsonrpc": "2.0", "method": "push_update", "params": {"identifier": {"scheme": "cusip", "value": {"cusip": "91282CRF0", "quote_currency": "%"}}, "source_timestamp": 1, "update": update}, "id": 0}),
        )
        .await;
        recv_json(&mut ws_stream).await;
        match tokio::time::timeout(Duration::from_millis(300), received.recv()).await {
            Ok(Some(batch)) if batch.contains(&1527) => break,
            Ok(Some(batch)) => panic!("unexpected feed ids {batch:?}"),
            Ok(None) => panic!("relayer channel closed"),
            Err(_elapsed) => assert!(
                tokio::time::Instant::now() < deadline,
                "CUSIP update never reached the relayer"
            ),
        }
    }

    let requests = [
        (
            serde_json::json!({"feed_id": 42, "source_timestamp": 1, "update": update}),
            "success",
        ),
        (
            serde_json::json!({"identifier": {"scheme": "cusip", "value": {"cusip": "91282CRF0", "quote_currency": "USD"}}, "source_timestamp": 1, "update": update}),
            "success",
        ),
        (
            serde_json::json!({"identifier": {"scheme": "cusip", "value": {"cusip": "000000000", "quote_currency": "%"}}, "source_timestamp": 1, "update": update}),
            "success",
        ),
        (
            serde_json::json!({"identifier": {"scheme": "cusip", "value": {"cusip": "91282CRF0"}}, "source_timestamp": 1, "update": update}),
            "parse error",
        ),
        (
            serde_json::json!({"source_timestamp": 1, "update": update}),
            "invalid params",
        ),
    ];
    for (id, (params, expected)) in (1..).zip(requests) {
        send_jrpc(
            &mut ws_sink,
            &serde_json::json!({"jsonrpc": "2.0", "method": "push_update", "params": params, "id": id}),
        )
        .await;
        let resp = recv_json(&mut ws_stream).await;
        match expected {
            "success" => {
                assert_eq!(resp["id"], id, "{resp}");
                assert_eq!(resp["result"], "success", "{resp}");
            }
            // A malformed request is answered without an id, as before this feature.
            "parse error" => assert_eq!(resp["error"]["code"], -32700, "{resp}"),
            _ => {
                assert_eq!(resp["id"], id, "{resp}");
                assert_eq!(resp["error"]["code"], -32602, "{resp}");
            }
        }
    }

    let mut feed_ids = std::collections::BTreeSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(feed_ids.contains(&42) && feed_ids.contains(&2903)) {
        let batch = tokio::time::timeout_at(deadline, received.recv())
            .await
            .expect("timeout waiting for relayer updates")
            .expect("relayer channel closed");
        feed_ids.extend(batch);
    }
    assert_eq!(feed_ids, std::collections::BTreeSet::from([42, 2903]));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), received.recv())
            .await
            .is_err(),
        "unexpected extra updates reached the relayer"
    );
}
