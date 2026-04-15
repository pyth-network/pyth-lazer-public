use std::time::Duration;

use pyth_lazer_client::backoff::PythLazerExponentialBackoffBuilder;
use pyth_lazer_client::hermes_client::PythLazerHermesClientBuilder;
use pyth_lazer_protocol::api::Channel;
use pyth_lazer_protocol::hermes::{HermesWsClientMessage, HermesWsServerMessage, PriceIdInput};
use pyth_lazer_protocol::time::FixedRate;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

fn get_lazer_access_token() -> String {
    // Place your access token in your env at LAZER_ACCESS_TOKEN or set it here
    let token = "your token here";
    std::env::var("LAZER_ACCESS_TOKEN").unwrap_or_else(|_| token.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()?,
        )
        .json()
        .init();

    // Optionally fetch available price feed metadata before subscribing
    let mut client = PythLazerHermesClientBuilder::new(get_lazer_access_token())
        // Optionally override the default endpoints
        .with_endpoints(vec![
            "wss://pyth-0.dourolabs.app".parse()?,
            "wss://pyth-1.dourolabs.app".parse()?,
        ])
        // Optionally set the number of connections
        .with_num_connections(4)
        // Optionally set the backoff strategy
        .with_backoff(PythLazerExponentialBackoffBuilder::default().build())
        // Optionally set the timeout for each connection
        .with_timeout(Duration::from_secs(5))
        // Optionally set the update channel / rate
        .with_channel(Channel::FixedRate(FixedRate::RATE_1000_MS))
        // Optionally set the channel capacity for responses
        .with_channel_capacity(1000)
        .build()?;

    // Fetch metadata for all available price feeds
    let metadata = client.fetch_metadata().await?;
    println!("Available price feeds ({} total):", metadata.len());

    let mut ids = Vec::new();
    for feed in metadata.iter().take(5) {
        println!("  id={:?} symbol={}", feed.id, feed.attributes.symbol);
        ids.push(PriceIdInput(feed.id.0.clone()));
    }

    let mut receiver = client.start().await?;

    // Subscribe to BTC/USD and ETH/USD price feeds using their Pyth price feed IDs.
    // Price IDs are 32-byte hex strings (with or without 0x prefix).
    // BTC/USD: e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43
    // ETH/USD: ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace
    client
        .send_message(HermesWsClientMessage::Subscribe {
            ids,
            verbose: true,
            binary: false,
            allow_out_of_order: false,
            ignore_invalid_price_ids: false,
        })
        .await?;

    // Process incoming messages
    let mut cnt = 0;
    while let Some(msg) = receiver.recv().await {
        cnt += 1;
        match msg {
            HermesWsServerMessage::Response(response) => {
                println!("Received response: {response:?}");
            }
            HermesWsServerMessage::PriceUpdate { price_feed } => {
                println!(
                    "Price update: id={:?} price={:?} ema={:?}",
                    price_feed.id, price_feed.price, price_feed.ema_price
                );
            }
        }
        if cnt >= 10 {
            break;
        }
    }

    Ok(())
}
