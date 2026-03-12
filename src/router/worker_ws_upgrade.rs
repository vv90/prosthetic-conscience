use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use tokio::sync::oneshot;
use tokio::time::{Instant, interval_at};
use tracing::{info, warn};

use crate::gateway::channel_registry::WorkerJob;
use crate::gateway::relay::{RelayOutcome, relay_job};
use crate::protocol::GatewayToWorker;
use crate::router::state::AppState;

pub(crate) async fn worker_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| worker_ws_connection(socket, state))
}

async fn worker_ws_connection(mut socket: WebSocket, state: AppState) {
    let (job_tx, job_rx) = oneshot::channel::<WorkerJob>();

    let mut worker_id = match state.runtime.register_worker(job_tx).await {
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
        worker_id = match state.runtime.register_worker(next_tx).await {
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
