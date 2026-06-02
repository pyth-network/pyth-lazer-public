#![allow(clippy::unwrap_used, clippy::disallowed_methods, reason = "test")]

//! Asserts the SDK's drop semantics. The receiver is the lifeline: tasks
//! keep running as long as someone is consuming from it. Dropping the
//! receiver winds the SDK down.
//!
//! Probe: point the SDK at a mock TCP server that counts accept attempts.
//! Tasks that are alive after drop keep trying to reconnect → the count
//! keeps going up. Tasks that exited cleanly → count stays flat.
//!
//! (An earlier version of this file used `runtime.shutdown_timeout` as the
//! signal, but for async-only runtimes tokio drops futures essentially
//! instantly during shutdown regardless of whether they had a cooperative
//! exit, so it was a false positive. TCP accept count is the reliable
//! probe for "task actually exited.")

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pyth_lazer_client::stream_client::{PythLazerStreamClient, PythLazerStreamClientBuilder};
use pyth_lazer_protocol::api::{
    Channel, DeliveryFormat, JsonBinaryEncoding, SubscribeRequest, SubscriptionId,
    SubscriptionParams, SubscriptionParamsRepr,
};
use pyth_lazer_protocol::{PriceFeedId, PriceFeedProperty};

struct MockServer {
    addr: std::net::SocketAddr,
    accept_count: Arc<AtomicUsize>,
}

async fn start_mock_server() -> MockServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accept_count = Arc::new(AtomicUsize::new(0));
    let cc = accept_count.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            cc.fetch_add(1, Ordering::Relaxed);
            drop(socket);
        }
    });
    MockServer { addr, accept_count }
}

fn build_client(addr: std::net::SocketAddr) -> PythLazerStreamClient {
    let url = format!("ws://{addr}/v1/stream").parse().unwrap();
    PythLazerStreamClientBuilder::new("test_token".to_string())
        .with_endpoints(vec![url])
        .with_num_connections(1)
        .with_timeout(Duration::from_secs(1))
        .build()
        .unwrap()
}

fn dummy_subscribe() -> SubscribeRequest {
    SubscribeRequest {
        subscription_id: SubscriptionId(1),
        params: SubscriptionParams::new(SubscriptionParamsRepr {
            price_feed_ids: Some(vec![PriceFeedId(8)]),
            symbols: None,
            properties: vec![PriceFeedProperty::Price],
            formats: vec![],
            delivery_format: DeliveryFormat::Json,
            json_binary_encoding: JsonBinaryEncoding::Base64,
            parsed: true,
            channel: Channel::RealTime,
            ignore_invalid_feeds: false,
        })
        .unwrap(),
    }
}

/// Receiver is the lifeline. Dropping the client while the receiver is still
/// alive must not interrupt the connection task — the receiver might still
/// want messages.
#[tokio::test]
async fn dropping_client_only_keeps_tasks_running() {
    let server = start_mock_server().await;
    let mut client = build_client(server.addr);
    let _receiver = client.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    drop(client);
    let before = server.accept_count.load(Ordering::Relaxed);

    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = server.accept_count.load(Ordering::Relaxed);
    let delta = after - before;

    assert!(
        delta >= 1,
        "tasks should keep reconnecting while the receiver is held; got delta={delta}"
    );
}

/// Dropping the receiver winds the SDK down: connection tasks notice via
/// `response_sender.closed()` and exit. Subsequent `subscribe()` calls
/// error because the per-connection request channel's receiver is gone.
#[tokio::test]
async fn dropping_receiver_stops_reconnects_and_blocks_subscribes() {
    let server = start_mock_server().await;
    let mut client = build_client(server.addr);
    let receiver = client.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    drop(receiver);
    // Let the closed() signal propagate to per-connection tasks.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let before = server.accept_count.load(Ordering::Relaxed);
    let subscribe_result = client.subscribe(dummy_subscribe()).await;

    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = server.accept_count.load(Ordering::Relaxed);
    let delta = after - before;

    assert_eq!(
        delta, 0,
        "tasks should have stopped reconnecting after receiver drop; got delta={delta}"
    );
    assert!(
        subscribe_result.is_err(),
        "subscribe after receiver drop should error; got {subscribe_result:?}"
    );
}

#[tokio::test]
async fn dropping_both_stops_reconnects() {
    let server = start_mock_server().await;
    let mut client = build_client(server.addr);
    let receiver = client.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    drop(client);
    drop(receiver);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let before = server.accept_count.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = server.accept_count.load(Ordering::Relaxed);
    let delta = after - before;

    assert_eq!(
        delta, 0,
        "tasks should have stopped reconnecting after both dropped; got delta={delta}"
    );
}
