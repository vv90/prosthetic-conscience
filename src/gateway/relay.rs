//! Job relay logic: reads worker websocket messages and forwards chunks
//! to the client stream channel. Reports terminal outcomes back to the
//! runtime via `RuntimeMessage::Event`.
//!
//! Designed to be called from the worker websocket handler as a regular
//! async function — no extra spawned task needed.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at};

use crate::gateway::channel_registry::{ClientStreamId, WorkerId};
use crate::gateway::runtime::RuntimeHandle;
use crate::protocol::WorkerMessage;

/// A single frame sent from the relay to the client SSE response handler.
#[derive(Debug, Clone)]
pub enum StreamFrame {
    Chunk { data: Value },
    Done,
    Error { message: String },
}

/// Outcome of a single relay run, to be translated into a kernel event
/// by the caller (the worker ws handler).
#[derive(Debug)]
pub enum RelayOutcome {
    /// Worker sent "end" — job completed normally.
    WorkerEnd,
    /// Worker sent "error" with a message.
    WorkerError { message: String },
    /// Worker websocket closed or errored before a terminal message.
    WorkerDisconnected,
    /// Client stream channel closed (client hung up) before job finished.
    ClientGone,
}

/// Relays a single job's worker output to a client stream channel.
///
/// Expects that the job frame has already been sent to the worker.
/// Reads worker websocket messages until a terminal message ("end"/"error")
/// or a connection failure, forwarding chunk data to `client_tx` along the way.
///
/// Returns a `RelayOutcome` that the caller maps to the appropriate kernel event.
pub async fn relay_job(
    socket: &mut WebSocket,
    client_tx: &mpsc::Sender<StreamFrame>,
    _worker_id: &WorkerId,
    client_stream_id: &ClientStreamId,
    runtime: &RuntimeHandle,
) -> RelayOutcome {
    let heartbeat_period = Duration::from_secs(10);
    let mut heartbeat_interval = interval_at(Instant::now() + heartbeat_period, heartbeat_period);

    loop {
        let msg_result = tokio::select! {
            msg = socket.recv() => {
                let Some(msg_result) = msg else {
                    return RelayOutcome::WorkerDisconnected;
                };
                msg_result
            }
            _ = heartbeat_interval.tick() => {
                let _ = runtime.stream_heartbeat(client_stream_id.clone()).await;
                continue;
            }
        };

        let text = match msg_result {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) => return RelayOutcome::WorkerDisconnected,
            Ok(_) => continue, // ping/pong/binary — skip
            Err(_) => return RelayOutcome::WorkerDisconnected,
        };

        let msg: WorkerMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(_) => continue, // malformed or unknown message type — skip
        };

        match msg {
            WorkerMessage::Chunk { data } => {
                let frame = StreamFrame::Chunk { data };
                if client_tx.send(frame).await.is_err() {
                    return RelayOutcome::ClientGone;
                }
            }
            WorkerMessage::End => {
                let _ = client_tx.send(StreamFrame::Done).await;
                return RelayOutcome::WorkerEnd;
            }
            WorkerMessage::Error { message } => {
                let _ = client_tx
                    .send(StreamFrame::Error {
                        message: message.clone(),
                    })
                    .await;
                return RelayOutcome::WorkerError { message };
            }
        }
    }
}
