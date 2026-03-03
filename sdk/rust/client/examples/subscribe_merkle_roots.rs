use std::time::Duration;

use pyth_lazer_client::backoff::PythLazerExponentialBackoffBuilder;
use pyth_lazer_client::merkle_stream_client::PythLazerMerkleStreamClientBuilder;
use tokio::pin;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

fn get_lazer_merkle_access_token() -> String {
    let token = "your token here";
    std::env::var("LAZER_MERKLE_ACCESS_TOKEN").unwrap_or_else(|_| token.to_string())
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

    let mut client = PythLazerMerkleStreamClientBuilder::new(get_lazer_merkle_access_token())
        .with_endpoints(vec![
            "wss://pyth-lazer-0.dourolabs.app/v1/merkle/root/stream".parse()?,
            "wss://pyth-lazer-1.dourolabs.app/v1/merkle/root/stream".parse()?,
        ])
        .with_num_connections(4)
        .with_backoff(PythLazerExponentialBackoffBuilder::default().build())
        .with_timeout(Duration::from_secs(5))
        .with_channel_capacity(1000)
        .build()?;

    let stream = client.start().await?;
    pin!(stream);

    println!("Connected to Merkle root stream. Waiting for updates...");

    let mut count = 0;
    while let Some(root) = stream.recv().await {
        println!(
            "Received SignedMerkleRoot: slot={}, timestamp={}, root={} bytes, signature={} bytes",
            root.slot,
            root.timestamp,
            root.root.len(),
            root.signature.len(),
        );

        count += 1;
        if count >= 50 {
            break;
        }
    }

    Ok(())
}
