#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use pyth_lazer_agent::config::Config;
use pyth_lazer_agent::http_server;
use pyth_lazer_agent::lazer_publisher::LazerPublisher;
use pyth_lazer_protocol::api::Channel;
use pyth_lazer_protocol::jrpc::SymbolMetadata;
use pyth_lazer_protocol::time::FixedRate;
use pyth_lazer_protocol::{PriceFeedId, SymbolState};
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use url::Url;

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
    let metadata = vec![SymbolMetadata {
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
    }];
    serde_json::to_string(&metadata).unwrap()
}

async fn start_mock_history_service() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let metadata_json = test_metadata_json();

    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let json = metadata_json.clone();
            tokio::spawn(async move {
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
                                        .body(http_body_util::Full::new(hyper::body::Bytes::from(
                                            json,
                                        )))
                                        .unwrap(),
                                )
                            }
                        }),
                    )
                    .await;
            });
        }
    });

    (port, handle)
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
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
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
    let (history_port, _history_handle) = start_mock_history_service().await;
    let history_url = Url::from_str(&format!("http://127.0.0.1:{history_port}")).unwrap();

    let signing_key_file = get_private_key_file();

    let agent_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent_port = agent_listener.local_addr().unwrap().port();
    drop(agent_listener);

    let config = Config {
        listen_address: format!("127.0.0.1:{agent_port}").parse().unwrap(),
        relayer_urls: vec![],
        authorization_token: None,
        publish_keypair_path: PathBuf::from(signing_key_file.path()),
        publish_interval_duration: Duration::from_millis(25),
        history_service_url: history_url,
        enable_update_deduplication: false,
        update_deduplication_ttl: Default::default(),
        proxy_url: None,
        legacy_sched_interval_duration: Duration::from_millis(200),
    };

    let lazer_publisher = LazerPublisher::new(&config).await;
    tokio::spawn(http_server::run(config, lazer_publisher));

    tokio::time::sleep(Duration::from_millis(100)).await;

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
    assert_eq!(products.len(), 1);
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
