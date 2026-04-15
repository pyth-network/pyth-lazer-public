use std::hash::{DefaultHasher, Hash, Hasher};

use anyhow::Result;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use pyth_lazer_protocol::api::Channel;
use pyth_lazer_protocol::hermes::{HermesWsClientMessage, HermesWsServerMessage};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

pub struct PythLazerHermesWSConnection {
    endpoint: Url,
    access_token: String,
    channel: Channel,
    ws_sender: Option<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    >,
}

/// Wrapper around server message for hashing/dedup.
#[derive(Debug, Clone, Hash)]
pub struct HermesServerMessageWrapper(pub HermesWsServerMessage);

impl HermesServerMessageWrapper {
    pub fn cache_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

impl PythLazerHermesWSConnection {
    pub fn new(endpoint: Url, access_token: String, channel: Channel) -> Result<Self> {
        Ok(Self {
            endpoint,
            access_token,
            channel,
            ws_sender: None,
        })
    }

    pub async fn start(
        &mut self,
    ) -> Result<impl futures_util::Stream<Item = Result<HermesServerMessageWrapper>> + use<>> {
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("channel", &self.channel.to_string());

        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url)?;

        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.access_token)
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid access token header value"))?,
        );

        let (ws_stream, _) = connect_async(request).await?;
        let (ws_sender, ws_receiver) = ws_stream.split();

        self.ws_sender = Some(ws_sender);
        let response_stream =
            ws_receiver
                .map_err(anyhow::Error::from)
                .try_filter_map(|msg| async {
                    let r: Result<Option<HermesServerMessageWrapper>> = match msg {
                        Message::Text(text) => {
                            let message = serde_json::from_str::<HermesWsServerMessage>(&text)?;
                            Ok(Some(HermesServerMessageWrapper(message)))
                        }
                        _ => Ok(None),
                    };
                    r
                });

        Ok(response_stream)
    }

    pub async fn send_message(&mut self, message: &HermesWsClientMessage) -> Result<()> {
        if let Some(sender) = &mut self.ws_sender {
            let msg = serde_json::to_string(message)?;
            sender.send(Message::Text(msg)).await?;
            Ok(())
        } else {
            anyhow::bail!("Hermes WebSocket connection not started")
        }
    }
}
