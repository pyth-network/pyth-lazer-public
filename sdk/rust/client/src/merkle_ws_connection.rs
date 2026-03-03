use std::hash::{DefaultHasher, Hash, Hasher};

use anyhow::Result;
use futures_util::{StreamExt, TryStreamExt};
use pyth_lazer_protocol::api::SignedMerkleRoot;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

pub struct PythLazerMerkleWSConnection {
    endpoint: Url,
    access_token: String,
}

pub fn cache_key(root: &SignedMerkleRoot) -> u64 {
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    hasher.finish()
}

impl PythLazerMerkleWSConnection {
    pub fn new(endpoint: Url, access_token: String) -> Result<Self> {
        Ok(Self {
            endpoint,
            access_token,
        })
    }

    pub async fn start(
        &mut self,
    ) -> Result<impl futures_util::Stream<Item = Result<SignedMerkleRoot>> + use<>> {
        let url = self.endpoint.clone();
        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url)?;

        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.access_token).parse().unwrap(),
        );

        let (ws_stream, _) = connect_async(request).await?;
        let (_ws_sender, ws_receiver) = ws_stream.split();

        let response_stream =
            ws_receiver
                .map_err(anyhow::Error::from)
                .try_filter_map(|msg| async {
                    let r: Result<Option<SignedMerkleRoot>> = match msg {
                        Message::Text(text) => {
                            Ok(Some(serde_json::from_str::<SignedMerkleRoot>(&text)?))
                        }
                        _ => Ok(None),
                    };
                    r
                });

        Ok(response_stream)
    }
}
