use {
    anyhow::{Context, Result, bail},
    pyth_lazer_protocol::{
        Price, PriceFeedId,
        jrpc::{FeedUpdateParams, JrpcId, JsonRpcVersion, PythLazerAgentJrpcV1},
        time::TimestampUs,
    },
    soketto::{
        Incoming, Sender,
        connection::Receiver,
        handshake::{Client, ServerResponse, client::Header},
    },
    std::{
        env::{self, VarError},
        time::Duration,
    },
    tokio::{net::TcpStream, time::sleep},
    tokio_util::compat::{Compat, TokioAsyncReadCompatExt},
    tracing::{Instrument, debug, error, info, info_span, level_filters::LevelFilter},
    tracing_subscriber::EnvFilter,
};

async fn try_connect(
    addr: &str,
) -> Result<(Sender<Compat<TcpStream>>, Receiver<Compat<TcpStream>>)> {
    // The address is set to work in tilt. TODO: make it configurable
    let socket = TcpStream::connect(addr).await?;
    socket.set_nodelay(true)?;
    let mut client = Client::new(socket.compat(), "...", "/v1/jrpc");
    client.set_headers(&[Header {
        name: "Authorization",
        value: b"Bearer example-publisher",
    }]);
    match client.handshake().await? {
        ServerResponse::Accepted { .. } => Ok(client.into_builder().finish()),
        ServerResponse::Redirect { .. } => bail!("unexpected redirect"),
        ServerResponse::Rejected { status_code } => bail!("rejected: {status_code:?}"),
    }
}

async fn try_connect_with_retry(
    addr: &str,
) -> Result<(Sender<Compat<TcpStream>>, Receiver<Compat<TcpStream>>)> {
    let max_retries = 5;
    let retry_delay = Duration::from_secs(2);

    for attempt in 1..=max_retries {
        info!("connecting (attempt {}/{})", attempt, max_retries);

        match try_connect(addr).await {
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
        Err(VarError::NotPresent) => "localhost:8910".to_string(),
        Err(VarError::NotUnicode(_)) => bail!("invalid env var value RELAYER_ADDR"),
    };

    let (mut sender, receiver) = try_connect_with_retry(&relayer_addr).await?;

    // Handle messages from the relayer, such as errors if we send a bad update
    for (i, mut receiver) in [receiver].into_iter().enumerate() {
        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                let mut data = Vec::new();
                loop {
                    data.clear();
                    match receiver.receive(&mut data).await {
                        Ok(Incoming::Data(_)) => info!(
                            "Received a message from relayer {i}: {}",
                            String::from_utf8_lossy(&data)
                        ),
                        Ok(Incoming::Pong(_)) => println!("Received a pong from relayer {i}"),
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

    let mut i = 0;

    let mut timer = TimestampUs::now();
    let mut update_counter = 0;
    let num_price_feeds = 5;
    let target_updates_per_second_per_feed = 2;
    let target_updates_per_second: u64 =
        (target_updates_per_second_per_feed * num_price_feeds).into();

    loop {
        i += 1;
        // let mut updates = Vec::new();
        for feed_id in 1u32..=num_price_feeds {
            let data = PythLazerAgentJrpcV1 {
                id: JrpcId::Null,
                jsonrpc: JsonRpcVersion::V2,
                params: pyth_lazer_protocol::jrpc::JrpcCall::PushUpdate(FeedUpdateParams {
                    feed_id: PriceFeedId(feed_id),
                    source_timestamp: TimestampUs::now(),
                    update: pyth_lazer_protocol::jrpc::UpdateParams::PriceUpdate {
                        price: Some(Price::from_integer((feed_id * 10000 + i).into(), -8)?),
                        best_bid_price: Some(Price::from_integer(
                            (feed_id * 10000 + i - 1).into(),
                            -8,
                        )?),
                        best_ask_price: Some(Price::from_integer(
                            (feed_id * 10000 + i + 1).into(),
                            -8,
                        )?),
                        trading_status: Some(pyth_lazer_protocol::api::TradingStatus::Open),
                        market_session: Some(pyth_lazer_protocol::api::MarketSession::Regular),
                    },
                }),
            };

            let buffer = serde_json::to_vec(&data)?;
            sender.send_binary(&buffer).await?;
            update_counter += 1;
            let time_passed = TimestampUs::now()
                .saturating_duration_since(timer)
                .as_micros();
            if time_passed * target_updates_per_second < update_counter * 1_000_000 {
                sender.flush().await?;
                sleep(Duration::from_micros(
                    update_counter * 1_000_000 / target_updates_per_second - time_passed,
                ))
                .await;
            }
            if time_passed >= 1_000_000 {
                let d: &[u8] = &[];
                sender.send_ping(d.try_into()?).await?;
                debug!("Sent {update_counter} updates in the last second");
                timer = TimestampUs::now();
                update_counter = 0;
            }
        }
    }
}
