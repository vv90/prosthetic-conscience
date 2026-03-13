use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::{error, info, warn};

use crate::protocol::{GatewayToWorker, WorkerMessage};
use crate::worker::inference::InferenceClient;

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub struct WorkerClient {
    gateway_url: String,
    inference: InferenceClient,
    auth_token: Option<String>,
}

impl WorkerClient {
    pub fn new(
        gateway_url: String,
        inference: InferenceClient,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            gateway_url,
            inference,
            auth_token,
        }
    }

    /// Build a WebSocket connection request, optionally with an auth header.
    fn build_request(&self) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
        let mut request = self
            .gateway_url
            .as_str()
            .into_client_request()
            .map_err(|e| format!("invalid gateway URL: {e}"))?;

        if let Some(token) = &self.auth_token {
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {token}")
                    .parse()
                    .map_err(|e| format!("invalid auth token: {e}"))?,
            );
        }

        Ok(request)
    }

    /// Run the worker loop forever: connect → process jobs → reconnect on failure.
    pub async fn run(&self) -> ! {
        let mut backoff = BACKOFF_INITIAL;

        loop {
            info!(url = %self.gateway_url, "connecting to gateway");

            let request = match self.build_request() {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "failed to build connection request");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            };

            match connect_async(request).await {
                Ok((ws, _response)) => {
                    info!("connected to gateway");
                    backoff = BACKOFF_INITIAL;

                    if let Err(msg) = self.job_loop(ws).await {
                        warn!(reason = %msg, "disconnected from gateway");
                    }
                }
                Err(e) => {
                    error!(error = %e, "failed to connect to gateway");
                }
            }

            info!(
                backoff_secs = backoff.as_secs(),
                "reconnecting after backoff"
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    /// Process jobs on an established WebSocket connection.
    ///
    /// Returns `Err(reason)` when the connection is lost and reconnection
    /// should be attempted.
    async fn job_loop(
        &self,
        ws: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Result<(), String> {
        let (mut sink, mut stream) = ws.split();

        loop {
            // Wait for a job message from the gateway.
            let job = loop {
                let msg = stream
                    .next()
                    .await
                    .ok_or_else(|| "gateway closed connection".to_string())?
                    .map_err(|e| format!("websocket read error: {e}"))?;

                match msg {
                    Message::Text(text) => {
                        let parsed: GatewayToWorker = serde_json::from_str(&text)
                            .map_err(|e| format!("failed to parse job message: {e}"))?;
                        break parsed;
                    }
                    Message::Close(_) => {
                        return Err("gateway sent close frame".to_string());
                    }
                    // Skip ping/pong/binary frames.
                    _ => continue,
                }
            };

            let GatewayToWorker::Job {
                client_stream_id,
                payload,
            } = job;

            info!(client_stream_id = %client_stream_id, "received job");

            // Stream inference and forward chunks to gateway.
            let result = self.process_job(&mut sink, payload).await;

            match &result {
                Ok(()) => {
                    info!(client_stream_id = %client_stream_id, "job completed");
                }
                Err(msg) => {
                    warn!(
                        client_stream_id = %client_stream_id,
                        error = %msg,
                        "job failed"
                    );
                }
            }

            // On WebSocket send failure, break to reconnect.
            // Inference errors were already sent as WorkerMessage::Error —
            // continue to next job.
            if let Err(msg) = result
                && msg.starts_with("websocket send error")
            {
                return Err(msg);
            }
        }
    }

    /// Run inference for a single job and stream results back.
    async fn process_job<S>(&self, sink: &mut S, payload: serde_json::Value) -> Result<(), String>
    where
        S: SinkExt<Message> + Unpin,
        S::Error: std::fmt::Display,
    {
        use futures_util::pin_mut;

        let chunk_stream = self.inference.stream_completion(payload);
        pin_mut!(chunk_stream);

        while let Some(result) = chunk_stream.next().await {
            match result {
                Ok(data) => {
                    let msg = WorkerMessage::Chunk { data };
                    let text = serde_json::to_string(&msg)
                        .map_err(|e| format!("serialization error: {e}"))?;
                    sink.send(Message::Text(text))
                        .await
                        .map_err(|e| format!("websocket send error: {e}"))?;
                }
                Err(e) => {
                    // Send error to gateway and return (job failed).
                    let msg = WorkerMessage::Error {
                        message: e.to_string(),
                    };
                    let text = serde_json::to_string(&msg)
                        .map_err(|e| format!("serialization error: {e}"))?;
                    sink.send(Message::Text(text))
                        .await
                        .map_err(|e| format!("websocket send error: {e}"))?;
                    return Err(format!("inference error: {e}"));
                }
            }
        }

        // Stream completed successfully — send end.
        let msg = WorkerMessage::End;
        let text = serde_json::to_string(&msg).map_err(|e| format!("serialization error: {e}"))?;
        sink.send(Message::Text(text))
            .await
            .map_err(|e| format!("websocket send error: {e}"))?;

        Ok(())
    }
}
