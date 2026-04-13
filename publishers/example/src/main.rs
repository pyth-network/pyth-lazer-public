use {
    anyhow::{Context, Result, bail},
    futures_util::{
        SinkExt, StreamExt,
        stream::{SplitSink, SplitStream},
    },
    pyth_lazer_protocol::{Price, PriceFeedId, publisher::PriceFeedDataV2, time::TimestampUs},
    std::{
        env::{self, VarError},
        time::Duration,
    },
    tokio::{net::TcpStream, time::sleep},
    tokio_tungstenite::{
        MaybeTlsStream, WebSocketStream, connect_async,
        tungstenite::{client::IntoClientRequest, http::header::HeaderValue, protocol::Message},
    },
    tracing::{Instrument, error, info, info_span, level_filters::LevelFilter},
    tracing_subscriber::EnvFilter,
    url::Url,
};

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type WsStream = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

fn build_url(addr: &str) -> Result<Url> {
    let mut url = if addr.starts_with("ws://") || addr.starts_with("wss://") {
        Url::parse(addr)?
    } else {
        // Maintain backward compatibility: allow bare host:port values.
        Url::parse(&format!("ws://{addr}"))?
    };

    if url.path() == "/" {
        url.set_path("/v2/publisher");
    }

    Ok(url)
}

async fn try_connect(addr: &str, access_token: &str) -> Result<(WsSink, WsStream)> {
    let url = build_url(addr)?;
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {access_token}"))?,
    );

    let (ws_stream, _response) = connect_async(request).await?;
    Ok(ws_stream.split())
}

async fn try_connect_with_retry(addr: &str, access_token: &str) -> Result<(WsSink, WsStream)> {
    let max_retries = 5;
    let retry_delay = Duration::from_secs(2);

    for attempt in 1..=max_retries {
        info!("connecting (attempt {}/{})", attempt, max_retries);

        match try_connect(addr, access_token).await {
            Ok(connection) => return Ok(connection),
            Err(error) => {
                info!(?error, "failed to connect");
                sleep(Duration::from_secs(5)).await;
            }
        }

        info!("retrying in {} seconds...", retry_delay.as_secs());
        sleep(retry_delay).await;
    }

    bail!("failed to connect after {} attempts", max_retries);
}

fn parse_env_u32(var_name: &str, default_value: u32) -> Result<u32> {
    match env::var(var_name) {
        Ok(value) => value
            .parse::<u32>()
            .with_context(|| format!("invalid env var value {var_name}")),
        Err(VarError::NotPresent) => Ok(default_value),
        Err(VarError::NotUnicode(_)) => bail!("invalid env var value {var_name}"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()
                .context("invalid RUST_LOG env var")?,
        )
        .json()
        .init();

    let relayer_addr = match env::var("RELAYER_ADDR") {
        Ok(v) => v,
        Err(VarError::NotPresent) => "localhost:10001".to_string(),
        Err(VarError::NotUnicode(_)) => bail!("invalid env var value RELAYER_ADDR"),
    };
    let relayer2_addr = match env::var("RELAYER2_ADDR") {
        Ok(v) => v,
        Err(VarError::NotPresent) => "localhost:10002".to_string(),
        Err(VarError::NotUnicode(_)) => bail!("invalid env var value RELAYER2_ADDR"),
    };

    let access_token = match env::var("PUBLISHER_ACCESS_TOKEN") {
        Ok(v) => v,
        Err(VarError::NotPresent) => "example-publisher".to_string(),
        Err(VarError::NotUnicode(_)) => bail!("invalid env var value PUBLISHER_ACCESS_TOKEN"),
    };

    let (mut sender, receiver) = try_connect_with_retry(&relayer_addr, &access_token).await?;
    let (mut sender2, receiver2) = try_connect_with_retry(&relayer2_addr, &access_token).await?;

    // Handle messages from the relayer, such as errors if we send a bad update
    for (i, stream) in [receiver, receiver2].into_iter().enumerate() {
        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                futures_util::pin_mut!(stream);
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            info!("Received a message from relayer {i}: {text}");
                        }
                        Ok(Message::Binary(data)) => {
                            info!(
                                "Received a message from relayer {i}: {}",
                                String::from_utf8_lossy(&data)
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            error!(?error, "Error receiving from relayer {i}");
                            break;
                        }
                    }
                }
            }
            .instrument(info_span!("relayer receive task", %i)),
        );
    }

    let mut buf = Vec::new();
    let mut i = 0;

    let mut timer = TimestampUs::now();
    let mut update_counter = 0;
    let num_price_feeds = parse_env_u32("NUM_PRICE_FEEDS", 5)?;
    let target_updates_per_second_per_feed =
        parse_env_u32("TARGET_UPDATES_PER_SECOND_PER_FEED", 2)?;
    let target_updates_per_second: u64 =
        (target_updates_per_second_per_feed * num_price_feeds).into();

    loop {
        i += 1;
        for feed_id in 1u32..=num_price_feeds {
            let data = PriceFeedDataV2 {
                price_feed_id: PriceFeedId(feed_id),
                source_timestamp_us: TimestampUs::now(),
                publisher_timestamp_us: TimestampUs::now(),
                price: Some(Price::from_integer((feed_id * 10000 + i).into(), -8)?),
                best_bid_price: Some(Price::from_integer((feed_id * 10000 + i - 1).into(), -8)?),
                best_ask_price: Some(Price::from_integer((feed_id * 10000 + i + 1).into(), -8)?),
                funding_rate: None,
            };
            buf.clear();
            bincode::serde::encode_into_std_write(data, &mut buf, bincode::config::legacy())?;
            sender.send(Message::Binary(buf.clone().into())).await?;
            sender2.send(Message::Binary(buf.clone().into())).await?;
            update_counter += 1;
            let time_passed = TimestampUs::now()
                .saturating_duration_since(timer)
                .as_micros();
            if time_passed * target_updates_per_second < update_counter * 1_000_000 {
                sender.flush().await?;
                sender2.flush().await?;
                sleep(Duration::from_micros(
                    update_counter * 1_000_000 / target_updates_per_second - time_passed,
                ))
                .await;
            }
            if time_passed >= 1_000_000 {
                info!("Sent {update_counter} updates in the last second");
                timer = TimestampUs::now();
                update_counter = 0;
            }
        }
    }
}
