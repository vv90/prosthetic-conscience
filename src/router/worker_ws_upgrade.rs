use std::collections::BTreeSet;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tokio::sync::oneshot;
use tokio::time::{Instant, interval_at};
use tracing::{info, warn};

use crate::gateway::channel_registry::WorkerJob;
use crate::gateway::relay::{RelayOutcome, relay_job};
use crate::protocol::GatewayToWorker;
use crate::protocol::parse_capabilities;
use crate::router::state::AppState;

/// Extract the `capabilities` query parameter value from a raw query string.
/// Expects format like `capabilities=chat,transcription`.
fn extract_capabilities_param(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == "capabilities" {
            Some(value)
        } else {
            None
        }
    })
}

pub(crate) async fn worker_ws_upgrade(
    ws: WebSocketUpgrade,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
    State(state): State<AppState>,
) -> Response {
    let capabilities_str = raw_query.as_deref().and_then(extract_capabilities_param);

    let capabilities = match capabilities_str {
        Some(s) => match parse_capabilities(s) {
            Ok(caps) => caps,
            Err(err) => {
                warn!(%err, "worker declared invalid capabilities");
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid capabilities: {err}"),
                )
                    .into_response();
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "missing capabilities query parameter",
            )
                .into_response();
        }
    };

    ws.on_upgrade(move |socket| worker_ws_connection(socket, state, capabilities))
        .into_response()
}

async fn worker_ws_connection(
    mut socket: WebSocket,
    state: AppState,
    capabilities: BTreeSet<crate::protocol::Capability>,
) {
    let (job_tx, job_rx) = oneshot::channel::<WorkerJob>();

    let mut worker_id = match state
        .runtime
        .register_worker(job_tx, capabilities.clone())
        .await
    {
        Ok(worker_id) => worker_id,
        Err(error) => {
            warn!(%error, "failed to register worker");
            return;
        }
    };

    info!(worker_id = %worker_id, "worker registered");

    // Wait for a job (or detect early websocket close)
    let mut pending_job = job_rx;
    let heartbeat_period = state.runtime.worker_heartbeat_interval;
    let mut heartbeat_interval = interval_at(Instant::now() + heartbeat_period, heartbeat_period);
    loop {
        let job = tokio::select! {
            result = &mut pending_job => {
                match result {
                    Ok(job) => job,
                    Err(_) => break, // runtime dropped our channel
                }
            }
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(_)) => continue, // ping/pong/binary while idle
                }
            }
            _ = heartbeat_interval.tick() => {
                if let Err(error) = state.runtime.worker_heartbeat(worker_id.clone()).await {
                    warn!(%error, worker_id = %worker_id, "failed to send heartbeat");
                    break;
                }
                continue;
            }
        };

        let client_stream_id = job.client_stream_id.clone();

        // Send job frame to worker
        let job_msg = GatewayToWorker::Job {
            client_stream_id: client_stream_id.to_string(),
            capability: job.capability,
            payload: job.payload,
        };
        let job_frame = match serde_json::to_string(&job_msg) {
            Ok(s) => s,
            Err(error) => {
                warn!(%error, worker_id = %worker_id, "failed to serialize job frame");
                break;
            }
        };

        if let Err(error) = socket.send(Message::Text(job_frame.into())).await {
            warn!(%error, worker_id = %worker_id, "failed to send job to worker");
            break;
        }

        let outcome = relay_job(
            &mut socket,
            &job.client_tx,
            &worker_id,
            &client_stream_id,
            &state.runtime,
            state.runtime.stream_heartbeat_interval,
        )
        .await;

        match outcome {
            RelayOutcome::WorkerEnd => {
                if let Err(error) = state.runtime.assignment_cleared(client_stream_id).await {
                    warn!(%error, worker_id = %worker_id, "failed to submit assignment cleared");
                    break;
                }
            }
            RelayOutcome::WorkerError { message } => {
                let _ = state
                    .runtime
                    .assignment_failed(client_stream_id, message)
                    .await;
            }
            RelayOutcome::WorkerDisconnected => {
                let _ = state
                    .runtime
                    .assignment_failed(
                        client_stream_id,
                        String::from("worker disconnected during job"),
                    )
                    .await;
                break; // worker is gone — exit the connection loop
            }
            RelayOutcome::ClientGone => {
                // Client hung up. The relay already stopped sending.
                // The kernel will time out the stream (or it was already
                // cleared by the client-side handler). Either way, the
                // worker connection is still alive — continue to re-register.
            }
        }

        // Re-register with a fresh ID and oneshot for the next job
        let (next_tx, next_rx) = oneshot::channel::<WorkerJob>();
        worker_id = match state
            .runtime
            .register_worker(next_tx, capabilities.clone())
            .await
        {
            Ok(id) => id,
            Err(error) => {
                warn!(%error, "failed to re-register worker");
                break;
            }
        };

        info!(worker_id = %worker_id, "worker re-registered");
        pending_job = next_rx;
        heartbeat_interval = interval_at(Instant::now() + heartbeat_period, heartbeat_period);
    }

    // No explicit unregister needed — cleanup is internally-driven.
    // Stale kernel/registry entries self-heal when dispatch fails.
    info!(worker_id = %worker_id, "worker disconnected");
}
