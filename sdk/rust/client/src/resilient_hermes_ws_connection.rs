use std::time::Duration;

use backoff::{ExponentialBackoff, backoff::Backoff};
use futures_util::StreamExt;
use pyth_lazer_protocol::{
    api::Channel,
    hermes::{HermesWsClientMessage, PriceIdInput},
};
use tokio::{pin, select, sync::mpsc, time::Instant};
use tracing::{Instrument, error, info, info_span};
use url::Url;

use crate::{
    CHANNEL_CAPACITY,
    hermes_ws_connection::{HermesServerMessageWrapper, PythLazerHermesWSConnection},
};
use anyhow::{Context, Result, bail};

const BACKOFF_RESET_DURATION: Duration = Duration::from_secs(10);

pub struct PythLazerResilientHermesWSConnection {
    request_sender: mpsc::Sender<HermesWsClientMessage>,
}

impl PythLazerResilientHermesWSConnection {
    pub fn new(
        endpoint: Url,
        access_token: String,
        channel: Channel,
        backoff: ExponentialBackoff,
        timeout: Duration,
        sender: mpsc::Sender<HermesServerMessageWrapper>,
    ) -> Self {
        let (request_sender, mut request_receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let mut task = PythLazerResilientHermesWSConnectionTask::new(
            endpoint.clone(),
            access_token,
            channel,
            backoff,
            timeout,
        );

        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                if let Err(e) = task.run(sender, &mut request_receiver).await {
                    error!("Resilient Hermes WebSocket connection task failed: {}", e);
                }
            }
            .instrument(info_span!("hermes ws connection task", %endpoint)),
        );

        Self { request_sender }
    }

    pub async fn send_message(&self, message: HermesWsClientMessage) -> Result<()> {
        self.request_sender
            .send(message)
            .await
            .context("Failed to send Hermes client message")?;
        Ok(())
    }
}

struct PythLazerResilientHermesWSConnectionTask {
    endpoint: Url,
    access_token: String,
    channel: Channel,
    // Only contains subscribe messages, used to res-subscribe on reconnects
    subscriptions: Vec<HermesWsClientMessage>,
    backoff: ExponentialBackoff,
    timeout: Duration,
}

impl PythLazerResilientHermesWSConnectionTask {
    pub fn new(
        endpoint: Url,
        access_token: String,
        channel: Channel,
        backoff: ExponentialBackoff,
        timeout: Duration,
    ) -> Self {
        Self {
            endpoint,
            access_token,
            channel,
            subscriptions: Vec::new(),
            backoff,
            timeout,
        }
    }

    pub async fn run(
        &mut self,
        response_sender: mpsc::Sender<HermesServerMessageWrapper>,
        request_receiver: &mut mpsc::Receiver<HermesWsClientMessage>,
    ) -> Result<()> {
        loop {
            let start_time = Instant::now();
            if let Err(e) = self.start(response_sender.clone(), request_receiver).await {
                if start_time.elapsed() > BACKOFF_RESET_DURATION
                    && start_time.elapsed() > self.timeout + Duration::from_secs(1)
                {
                    self.backoff.reset();
                }

                let delay = self.backoff.next_backoff();
                match delay {
                    Some(d) => {
                        info!(
                            channel = %self.channel,
                            "Hermes WebSocket connection failed: {}. Retrying in {:?}",
                            e, d
                        );
                        tokio::time::sleep(d).await;
                    }
                    None => {
                        bail!(
                            "Max retries reached for Hermes WebSocket connection to {}, this should never happen, please contact developers",
                            self.endpoint
                        );
                    }
                }
            }
        }
    }

    pub async fn start(
        &mut self,
        sender: mpsc::Sender<HermesServerMessageWrapper>,
        request_receiver: &mut mpsc::Receiver<HermesWsClientMessage>,
    ) -> Result<()> {
        let mut ws_connection = PythLazerHermesWSConnection::new(
            self.endpoint.clone(),
            self.access_token.clone(),
            self.channel,
        )?;
        let stream = ws_connection.start().await?;
        pin!(stream);

        for subscription in self.subscriptions.clone() {
            ws_connection.send_message(&subscription).await?;
        }

        loop {
            let timeout_response = tokio::time::timeout(self.timeout, stream.next());

            select! {
                response = timeout_response => {
                    match response {
                        Ok(Some(response)) => match response {
                            Ok(response) => {
                                sender
                                    .send(response)
                                    .await
                                    .context("Failed to send Hermes response")?;
                            }
                            Err(e) => {
                                bail!("Hermes WebSocket stream error: {}", e);
                            }
                        },
                        Ok(None) => {
                            bail!("Hermes WebSocket stream ended unexpectedly");
                        }
                        Err(_elapsed) => {
                            bail!("Hermes WebSocket stream timed out");
                        }
                    }
                }
                Some(request) = request_receiver.recv() => {
                    self.handle_request(&mut ws_connection, request).await?;
                }
            }
        }
    }

    async fn handle_request(
        &mut self,
        ws_connection: &mut PythLazerHermesWSConnection,
        request: HermesWsClientMessage,
    ) -> Result<()> {
        let ids = subscription_ids(&request);

        // In both subscribe and unsubscribe cases, we need to remove the ids from existing subscriptions
        self.subscriptions
            .iter_mut()
            .for_each(|sub| remove_ids_from_subscription(sub, &ids));

        // Remove any subscriptions that have no more ids after the above removal
        self.subscriptions.retain(|sub| !is_subscription_empty(sub));

        // If it's a subscribe message, we need to add it to the list of subscriptions
        if matches!(request, HermesWsClientMessage::Subscribe { .. }) {
            self.subscriptions.push(request.clone());
        }

        // Send the request to WS connection
        ws_connection.send_message(&request).await
    }
}

fn remove_ids_from_subscription(sub: &mut HermesWsClientMessage, ids_to_remove: &[PriceIdInput]) {
    match sub {
        HermesWsClientMessage::Subscribe { ids, .. } => {
            ids.retain(|id| !ids_to_remove.contains(id));
        }
        HermesWsClientMessage::Unsubscribe { .. } => (),
    }
}

fn subscription_ids(sub: &HermesWsClientMessage) -> Vec<PriceIdInput> {
    match sub {
        HermesWsClientMessage::Subscribe { ids, .. } => ids.clone(),
        HermesWsClientMessage::Unsubscribe { ids } => ids.clone(),
    }
}

fn is_subscription_empty(sub: &HermesWsClientMessage) -> bool {
    match sub {
        HermesWsClientMessage::Subscribe { ids, .. } => ids.is_empty(),
        HermesWsClientMessage::Unsubscribe { ids } => ids.is_empty(),
    }
}
