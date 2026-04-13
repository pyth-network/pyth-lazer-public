use {
    std::time::Duration,
    tracing::{Instrument, info_span},
};

use backoff::{ExponentialBackoff, backoff::Backoff};
use futures_util::StreamExt;
use pyth_lazer_protocol::api::SignedMerkleRoot;
use tokio::{pin, sync::mpsc, time::Instant};
use tracing::{error, info};
use url::Url;

use crate::merkle_ws_connection::PythLazerMerkleWSConnection;
use anyhow::{Result, bail};

const BACKOFF_RESET_DURATION: Duration = Duration::from_secs(10);

pub struct PythLazerResilientMerkleWSConnection;

impl PythLazerResilientMerkleWSConnection {
    pub fn new(
        endpoint: Url,
        access_token: String,
        backoff: ExponentialBackoff,
        timeout: Duration,
        sender: mpsc::Sender<SignedMerkleRoot>,
    ) -> Self {
        let mut task = PythLazerResilientMerkleWSConnectionTask::new(
            endpoint.clone(),
            access_token,
            backoff,
            timeout,
        );

        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                if let Err(e) = task.run(sender).await {
                    error!("Resilient Merkle WebSocket connection task failed: {}", e);
                }
            }
            .instrument(info_span!("merkle ws connection task", %endpoint)),
        );

        Self
    }
}

struct PythLazerResilientMerkleWSConnectionTask {
    endpoint: Url,
    access_token: String,
    backoff: ExponentialBackoff,
    timeout: Duration,
}

impl PythLazerResilientMerkleWSConnectionTask {
    pub fn new(
        endpoint: Url,
        access_token: String,
        backoff: ExponentialBackoff,
        timeout: Duration,
    ) -> Self {
        Self {
            endpoint,
            access_token,
            backoff,
            timeout,
        }
    }

    pub async fn run(&mut self, response_sender: mpsc::Sender<SignedMerkleRoot>) -> Result<()> {
        loop {
            let start_time = Instant::now();
            if let Err(e) = self.start(response_sender.clone()).await {
                if start_time.elapsed() > BACKOFF_RESET_DURATION
                    && start_time.elapsed() > self.timeout + Duration::from_secs(1)
                {
                    self.backoff.reset();
                }

                let delay = self.backoff.next_backoff();
                match delay {
                    Some(d) => {
                        info!(
                            "Merkle WebSocket connection failed: {}. Retrying in {:?}",
                            e, d
                        );
                        tokio::time::sleep(d).await;
                    }
                    None => {
                        bail!(
                            "Max retries reached for Merkle WebSocket connection to {}, this should never happen, please contact developers",
                            self.endpoint
                        );
                    }
                }
            }
        }
    }

    async fn start(&mut self, sender: mpsc::Sender<SignedMerkleRoot>) -> Result<()> {
        let mut ws_connection =
            PythLazerMerkleWSConnection::new(self.endpoint.clone(), self.access_token.clone())?;
        let stream = ws_connection.start().await?;
        pin!(stream);

        loop {
            match tokio::time::timeout(self.timeout, stream.next()).await {
                Ok(Some(response)) => match response {
                    Ok(root) => {
                        sender
                            .send(root)
                            .await
                            .map_err(|_| anyhow::anyhow!("Failed to send response"))?;
                    }
                    Err(e) => {
                        bail!("Merkle WebSocket stream error: {}", e);
                    }
                },
                Ok(None) => {
                    bail!("Merkle WebSocket stream ended unexpectedly");
                }
                Err(_elapsed) => {
                    bail!("Merkle WebSocket stream timed out");
                }
            }
        }
    }
}
