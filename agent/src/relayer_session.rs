use crate::config::RedactedUrl;
use anyhow::{Context, Result, bail};
use backoff::backoff::Backoff;
use backoff::{ExponentialBackoff, ExponentialBackoffBuilder};
use base64::Engine;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use http::HeaderValue;
use protobuf::Message;
use pyth_lazer_publisher_sdk::transaction::SignedLazerTransaction;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::select;
use tokio::sync::{Semaphore, SemaphorePermit, broadcast};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async, connect_async_with_config,
    tungstenite::Message as TungsteniteMessage,
};
use tracing::Instrument;
use url::Url;

type RelayerWsSender = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, TungsteniteMessage>;
type RelayerWsReceiver = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

async fn connect_through_proxy(
    proxy_url: &RedactedUrl,
    target_url: &Url,
    token: &str,
) -> Result<(RelayerWsSender, RelayerWsReceiver)> {
    // `proxy_url`'s Display/Debug are redacted, so it is safe to log directly.
    // `proxy` carries the credentials and is only used to build the connection.
    let proxy = proxy_url.expose();

    tracing::info!(
        "connecting to the relayer at {} via proxy {}",
        target_url,
        proxy_url
    );

    let proxy_host = proxy.host_str().context("Proxy URL must have a host")?;
    let proxy_port = proxy
        .port()
        .unwrap_or(if proxy.scheme() == "https" { 443 } else { 80 });

    let proxy_addr = format!("{proxy_host}:{proxy_port}");
    let mut stream = TcpStream::connect(&proxy_addr)
        .await
        .context(format!("Failed to connect to proxy at {proxy_addr}"))?;

    let target_host = target_url
        .host_str()
        .context("Target URL must have a host")?;
    let target_port = target_url
        .port()
        .unwrap_or(if target_url.scheme() == "wss" {
            443
        } else {
            80
        });

    let target_authority = format!("{target_host}:{target_port}");
    let mut request_parts = vec![format!("CONNECT {target_authority} HTTP/1.1")];
    request_parts.push(format!("Host: {target_authority}"));

    let username = proxy.username();
    if !username.is_empty() {
        let password = proxy.password().unwrap_or("");
        let credentials = format!("{username}:{password}");
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
        request_parts.push(format!("Proxy-Authorization: Basic {encoded}"));
    }

    request_parts.push("Proxy-Connection: Keep-Alive".to_string());
    request_parts.push(String::new()); // Empty line to end headers
    request_parts.push(String::new()); // CRLF to end request

    let connect_request = request_parts.join("\r\n");

    stream
        .write_all(connect_request.as_bytes())
        .await
        .context(format!(
            "Failed to send CONNECT request to proxy at {proxy_url}"
        ))?;

    let mut response_buffer = Vec::new();
    let mut temp_buf = [0u8; 1024];
    let mut headers_complete = false;

    while !headers_complete {
        let n = stream.read(&mut temp_buf).await.context(format!(
            "Failed to read CONNECT response from proxy at {proxy_url}"
        ))?;

        if n == 0 {
            bail!("Proxy closed connection before sending complete response");
        }

        response_buffer.extend_from_slice(temp_buf.get(..n).context("Invalid buffer slice")?);

        if response_buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            headers_complete = true;
        }
    }

    let response_str = String::from_utf8_lossy(&response_buffer);

    let status_line = response_str
        .lines()
        .next()
        .context("Empty response from proxy")?;

    let parts: Vec<&str> = status_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!(
            "Invalid HTTP response from proxy at {}: {}",
            proxy_url,
            status_line
        );
    }

    let status_code = parts
        .get(1)
        .context("Missing status code in proxy response")?
        .parse::<u16>()
        .context("Invalid status code in proxy response")?;

    if status_code != 200 {
        let status_text = parts
            .get(2..)
            .map(|s| s.join(" "))
            .unwrap_or_else(|| "Unknown".to_string());
        bail!(
            "Proxy CONNECT failed with status {} {}: {}",
            status_code,
            status_text,
            status_line
        );
    }

    tracing::info!("Successfully connected through proxy at {}", proxy_url);

    let mut req = target_url.clone().into_client_request()?;
    let headers = req.headers_mut();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    headers.insert(
        "X-Agent-Version",
        HeaderValue::from_str(env!("CARGO_PKG_VERSION"))?,
    );

    let maybe_tls_stream = if target_url.scheme() == "wss" {
        let tls_connector = tokio_native_tls::native_tls::TlsConnector::builder()
            .build()
            .context("Failed to build TLS connector")?;
        let tokio_connector = tokio_native_tls::TlsConnector::from(tls_connector);
        let domain = target_host;
        let tls_stream = tokio_connector
            .connect(domain, stream)
            .await
            .context("Failed to establish TLS connection")?;

        MaybeTlsStream::NativeTls(tls_stream)
    } else {
        MaybeTlsStream::Plain(stream)
    };

    let (ws_stream, _) = client_async(req, maybe_tls_stream)
        .await
        .context("Failed to complete WebSocket handshake")?;

    tracing::info!(
        "WebSocket connection established to relayer at {} via proxy {}",
        target_url,
        proxy_url
    );
    Ok(ws_stream.split())
}

async fn connect_to_relayer(
    url: Url,
    token: &str,
    proxy_url: Option<&RedactedUrl>,
) -> Result<(RelayerWsSender, RelayerWsReceiver)> {
    if let Some(proxy) = proxy_url {
        connect_through_proxy(proxy, &url, token).await
    } else {
        tracing::info!("connecting to the relayer at {}", url);
        let mut req = url.clone().into_client_request()?;
        let headers = req.headers_mut();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        headers.insert(
            "X-Agent-Version",
            HeaderValue::from_str(env!("CARGO_PKG_VERSION"))?,
        );
        let (ws_stream, _) = connect_async_with_config(req, None, true).await?;
        tracing::info!("connected to the relayer at {}", url);
        Ok(ws_stream.split())
    }
}

struct RelayerWsSession {
    ws_sender: RelayerWsSender,
}

impl RelayerWsSession {
    async fn send_transaction(
        &mut self,
        signed_lazer_transaction: SignedLazerTransaction,
    ) -> Result<()> {
        tracing::debug!(
            "Sending SignedLazerTransaction: {:?}",
            signed_lazer_transaction
        );
        let buf = signed_lazer_transaction.write_to_bytes()?;
        self.ws_sender
            .send(TungsteniteMessage::from(buf.clone()))
            .await?;
        self.ws_sender.flush().await?;
        Ok(())
    }
}

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
/// A connection that stayed up at least this long is treated as healthy, so the
/// next reconnect skips backoff (the drop was likely a relayer restart).
const STABLE_CONNECTION: Duration = Duration::from_secs(30);
/// Cap on how long a single connect attempt may block before we give up and let
/// the worker move on to another relayer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

enum ConnectionOutcome {
    /// Never established a connection.
    ConnectFailed,
    /// Connected, then the connection ended after being up for `uptime`.
    Disconnected { uptime: Duration },
}

struct SlotState {
    backoff: ExponentialBackoff,
    /// Earliest instant this relayer may be attempted again.
    ready_at: Instant,
}

/// One relayer in the pool. `permit` (capacity 1) is held by the worker
/// currently using this relayer, so at most one worker connects to it at a time.
pub struct RelayerSlot {
    url: Url,
    permit: Semaphore,
    state: Mutex<SlotState>,
}

impl RelayerSlot {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            permit: Semaphore::new(1),
            state: Mutex::new(SlotState {
                backoff: ExponentialBackoffBuilder::new()
                    .with_initial_interval(INITIAL_BACKOFF)
                    .with_max_interval(MAX_BACKOFF)
                    .with_max_elapsed_time(None)
                    .build(),
                ready_at: Instant::now(),
            }),
        }
    }

    fn ready_at(&self) -> Instant {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ready_at
    }

    /// Update this relayer's backoff/deadline from how its last attempt went.
    fn record_outcome(&self, outcome: ConnectionOutcome) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match outcome {
            ConnectionOutcome::Disconnected { uptime } if uptime >= STABLE_CONNECTION => {
                state.backoff.reset();
                state.ready_at = Instant::now();
                drop(state);
                tracing::info!(
                    "relayer {} disconnected after {uptime:?}; reconnecting",
                    self.url
                );
            }
            _ => {
                let next = state.backoff.next_backoff().unwrap_or(MAX_BACKOFF);
                state.ready_at = Instant::now() + next;
                drop(state);
                tracing::warn!("relayer {} unavailable; next attempt in {next:?}", self.url);
            }
        }
    }
}

/// Fixed set of relayer slots shared by all worker tasks.
pub type RelayerPool = Arc<Vec<RelayerSlot>>;

pub struct RelayerSessionTask {
    pub pool: RelayerPool,
    pub token: String,
    pub is_ready: Arc<AtomicBool>,
    pub proxy_url: Option<RedactedUrl>,
}

impl RelayerSessionTask {
    pub async fn run(&self, mut receiver: broadcast::Receiver<SignedLazerTransaction>) {
        loop {
            let Some((permit, slot)) = self.reserve_slot().await else {
                // Every relayer is currently held by another worker.
                tokio::time::sleep(INITIAL_BACKOFF).await;
                continue;
            };

            // Respect this relayer's backoff deadline before attempting.
            let wait = slot.ready_at().saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }

            let outcome = self
                .run_relayer_connection(&slot.url, &mut receiver)
                .instrument(tracing::info_span!("relayer_connection", url = %slot.url))
                .await;
            slot.record_outcome(outcome);
            drop(permit);
        }
    }

    /// Reserve the unheld relayer whose backoff deadline is soonest, returning
    /// the held permit and the slot. `None` if every relayer is already held.
    async fn reserve_slot(&self) -> Option<(SemaphorePermit<'_>, &RelayerSlot)> {
        loop {
            let mut best: Option<(usize, Instant)> = None;
            for (i, slot) in self.pool.iter().enumerate() {
                if slot.permit.available_permits() == 0 {
                    continue; // held by another worker
                }
                let ready_at = slot.ready_at();
                if best.is_none_or(|(_, best_ready)| ready_at < best_ready) {
                    best = Some((i, ready_at));
                }
            }

            let (idx, _) = best?;
            let slot = self.pool.get(idx)?;
            if let Ok(permit) = slot.permit.try_acquire() {
                return Some((permit, slot));
            }
            // Another worker claimed it between the scan and acquire; re-scan.
            tokio::task::yield_now().await;
        }
    }

    async fn run_relayer_connection(
        &self,
        url: &Url,
        receiver: &mut broadcast::Receiver<SignedLazerTransaction>,
    ) -> ConnectionOutcome {
        let connect = tokio::time::timeout(
            CONNECT_TIMEOUT,
            connect_to_relayer(url.clone(), &self.token, self.proxy_url.as_ref()),
        )
        .await;

        let (relayer_ws_sender, mut relayer_ws_receiver) = match connect {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                tracing::warn!("failed to connect to relayer {url}: {e:?}");
                return ConnectionOutcome::ConnectFailed;
            }
            Err(_) => {
                tracing::warn!("timed out connecting to relayer {url} after {CONNECT_TIMEOUT:?}");
                return ConnectionOutcome::ConnectFailed;
            }
        };
        let mut relayer_ws_session = RelayerWsSession {
            ws_sender: relayer_ws_sender,
        };

        // If we have at least one successful connection, mark as ready.
        self.is_ready.store(true, Ordering::Relaxed);
        let connected_at = Instant::now();

        loop {
            select! {
                recv_result = receiver.recv() => {
                    match recv_result {
                        Ok(transaction) => {
                            if let Err(e) = relayer_ws_session.send_transaction(transaction).await {
                                tracing::error!("Error publishing transaction to relayer {url}: {e:?}");
                                break;
                            }
                        },
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::error!("transaction broadcast channel closed");
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped_count)) => {
                            tracing::warn!("transaction broadcast channel lagged by {skipped_count} messages");
                        }
                    }
                }
                // Handle messages from the relayers, such as errors if we send a bad update
                msg = relayer_ws_receiver.next() => {
                    match msg {
                        Some(Ok(msg)) => {
                            tracing::debug!("Received a message from relayer: {msg:?}");
                        }
                        Some(Err(e)) => {
                            tracing::error!("Error receiving message from relayer {url}: {e:?}");
                        }
                        None => {
                            tracing::warn!("relayer connection closed url: {url}");
                            break;
                        }
                    }
                }
            }
        }

        ConnectionOutcome::Disconnected {
            uptime: connected_at.elapsed(),
        }
    }
}

//noinspection DuplicatedCode
#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use futures_util::StreamExt;
    use protobuf::well_known_types::timestamp::Timestamp;
    use protobuf::{Message, MessageField};
    use pyth_lazer_publisher_sdk::publisher_update::feed_update::Update;
    use pyth_lazer_publisher_sdk::publisher_update::{FeedUpdate, PriceUpdate, PublisherUpdate};
    use pyth_lazer_publisher_sdk::transaction::lazer_transaction::Payload;
    use pyth_lazer_publisher_sdk::transaction::signature_data::Data::Ed25519;
    use pyth_lazer_publisher_sdk::transaction::{
        Ed25519SignatureData, LazerTransaction, SignatureData, SignedLazerTransaction,
    };
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};
    use tokio::net::TcpListener;
    use tokio::sync::{broadcast, mpsc};
    use url::Url;
    use {
        crate::relayer_session::{
            ConnectionOutcome, RelayerPool, RelayerSessionTask, RelayerSlot, STABLE_CONNECTION,
        },
        tracing::{Instrument, info_span},
    };

    pub const RELAYER_CHANNEL_CAPACITY: usize = 1000;

    fn get_private_key() -> SigningKey {
        SigningKey::from_keypair_bytes(&[
            105, 175, 146, 91, 32, 145, 164, 199, 37, 111, 139, 255, 44, 225, 5, 247, 154, 170,
            238, 70, 47, 15, 9, 48, 102, 87, 180, 50, 50, 38, 148, 243, 62, 148, 219, 72, 222, 170,
            8, 246, 176, 33, 205, 29, 118, 11, 220, 163, 214, 204, 46, 49, 132, 94, 170, 173, 244,
            39, 179, 211, 177, 70, 252, 31,
        ])
        .unwrap()
    }

    pub async fn run_mock_relayer(
        addr: SocketAddr,
        back_sender: mpsc::Sender<SignedLazerTransaction>,
    ) {
        let listener = TcpListener::bind(addr).await.unwrap();

        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                let Ok((stream, _)) = listener.accept().await else {
                    panic!("failed to accept mock relayer websocket connection");
                };
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("handshake failed");
                let (_, mut read) = ws_stream.split();
                while let Some(msg) = read.next().await {
                    if let Ok(msg) = msg {
                        if msg.is_binary() {
                            tracing::info!("Received a binary message: {msg:?}");
                            let transaction =
                                SignedLazerTransaction::parse_from_bytes(msg.into_data().as_ref())
                                    .unwrap();
                            back_sender.clone().send(transaction).await.unwrap();
                        }
                    } else {
                        tracing::error!("Received a malformed message: {msg:?}");
                    }
                }
            }
            .instrument(info_span!("mock relayer")),
        );
    }

    #[tokio::test]
    async fn test_relayer_session() {
        let (back_sender, mut back_receiver) = mpsc::channel(RELAYER_CHANNEL_CAPACITY);
        let relayer_addr = "127.0.0.1:12346".parse().unwrap();
        run_mock_relayer(relayer_addr, back_sender).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let (relayer_sender, _) = broadcast::channel(RELAYER_CHANNEL_CAPACITY);
        let pool: RelayerPool = Arc::new(vec![RelayerSlot::new(
            Url::parse("ws://127.0.0.1:12346").unwrap(),
        )]);

        let relayer_session_task = RelayerSessionTask {
            pool,
            token: "token1".to_string(),
            is_ready: Arc::new(AtomicBool::new(false)),
            proxy_url: None,
        };
        let receiver = relayer_sender.subscribe();
        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move { relayer_session_task.run(receiver).await }
                .instrument(info_span!("relayer session task")),
        );
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        let transaction = get_signed_lazer_transaction();
        relayer_sender
            .send(transaction.clone())
            .expect("relayer_sender.send failed");
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let received_transaction = back_receiver
            .recv()
            .await
            .expect("back_receiver.recv failed");
        assert_eq!(transaction, received_transaction);
    }

    fn set_ready_at(slot: &RelayerSlot, at: Instant) {
        slot.state.lock().unwrap().ready_at = at;
    }

    #[test]
    fn test_slot_backoff_schedule() {
        let slot = RelayerSlot::new(Url::parse("ws://relayer").unwrap());
        // A fresh slot is attemptable immediately.
        assert!(slot.ready_at() <= Instant::now());

        // A failed connect pushes the deadline into the future.
        slot.record_outcome(ConnectionOutcome::ConnectFailed);
        assert!(slot.ready_at() > Instant::now());

        // A flapping (short-lived) connection also backs off.
        slot.record_outcome(ConnectionOutcome::Disconnected {
            uptime: Duration::from_millis(1),
        });
        assert!(slot.ready_at() > Instant::now());

        // A stable connection that drops resets backoff: retry promptly.
        slot.record_outcome(ConnectionOutcome::Disconnected {
            uptime: STABLE_CONNECTION,
        });
        assert!(slot.ready_at() <= Instant::now() + Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_reserve_slot_prefers_soonest_and_excludes_held() {
        let url_a = Url::parse("ws://relayer-a").unwrap();
        let url_b = Url::parse("ws://relayer-b").unwrap();
        let pool: RelayerPool = Arc::new(vec![
            RelayerSlot::new(url_a.clone()),
            RelayerSlot::new(url_b.clone()),
        ]);
        // B is ready now; A is parked far in the future.
        set_ready_at(&pool[0], Instant::now() + Duration::from_secs(60));
        set_ready_at(&pool[1], Instant::now());

        let task = RelayerSessionTask {
            pool: pool.clone(),
            token: "token".to_string(),
            is_ready: Arc::new(AtomicBool::new(false)),
            proxy_url: None,
        };

        // Soonest deadline wins.
        let (permit_b, slot) = task.reserve_slot().await.expect("should reserve B");
        assert_eq!(slot.url, url_b);

        // B is held, so the only remaining choice is A.
        let (permit_a, slot) = task.reserve_slot().await.expect("should reserve A");
        assert_eq!(slot.url, url_a);

        // Both held: nothing left to reserve.
        assert!(task.reserve_slot().await.is_none());

        drop(permit_b);
        drop(permit_a);
    }

    fn get_signed_lazer_transaction() -> SignedLazerTransaction {
        let publisher_update = PublisherUpdate {
            updates: vec![FeedUpdate {
                feed_id: Some(1),
                source_timestamp: MessageField::some(Timestamp::now()),
                update: Some(Update::PriceUpdate(PriceUpdate {
                    price: Some(1_000_000_000i64),
                    ..PriceUpdate::default()
                })),
                special_fields: Default::default(),
            }],
            publisher_timestamp: MessageField::some(Timestamp::now()),
            special_fields: Default::default(),
        };
        let lazer_transaction = LazerTransaction {
            payload: Some(Payload::PublisherUpdate(publisher_update)),
            special_fields: Default::default(),
        };
        let buf = lazer_transaction.write_to_bytes().unwrap();
        let signing_key = get_private_key();
        let signature = signing_key.sign(&buf);
        let signature_data = SignatureData {
            data: Some(Ed25519(Ed25519SignatureData {
                signature: Some(signature.to_bytes().into()),
                public_key: Some(signing_key.verifying_key().to_bytes().into()),
                special_fields: Default::default(),
            })),
            special_fields: Default::default(),
        };
        SignedLazerTransaction {
            signature_data: MessageField::some(signature_data),
            payload: Some(buf),
            special_fields: Default::default(),
        }
    }
}
