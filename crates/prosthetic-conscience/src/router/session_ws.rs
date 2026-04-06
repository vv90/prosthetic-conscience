use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at};
use tracing::{info, warn};

use crate::gateway::kernel::SessionId;
use crate::protocol::{SessionClientMessage, SessionGatewayMessage};
use crate::router::state::AppState;

pub(crate) async fn session_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| session_ws_connection(socket, state))
}

async fn session_ws_connection(mut socket: WebSocket, state: AppState) {
    // 1. Wait for handshake message (with timeout)
    let handshake_timeout = Duration::from_secs(5);
    let first_msg = match tokio::time::timeout(handshake_timeout, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => return,
    };

    let handshake = match serde_json::from_str::<SessionClientMessage>(&first_msg) {
        Ok(msg @ SessionClientMessage::Create)
        | Ok(msg @ SessionClientMessage::Subscribe { .. }) => msg,
        _ => {
            let err = SessionGatewayMessage::Error {
                message: String::from("expected create or subscribe as first message"),
            };
            if let Ok(json) = serde_json::to_string(&err) {
                let _ = socket.send(Message::Text(json.into())).await;
            }
            return;
        }
    };

    // 2. Create subscriber channel
    let (tx, mut rx) = mpsc::channel::<SessionGatewayMessage>(32);

    // 3. Register and execute handshake
    let (subscriber_id, session_id) = match handshake {
        SessionClientMessage::Create => {
            let subscriber_id = match state.runtime.session_create(tx).await {
                Ok(id) => id,
                Err(err) => {
                    warn!(%err, "failed to create session");
                    return;
                }
            };
            // Wait for Subscribed from mpsc (sent by SessionCreated effect executor)
            let subscribed = match rx.recv().await {
                Some(SessionGatewayMessage::Subscribed {
                    session_id,
                    latest_entry_index,
                }) => (SessionId(session_id.clone()), latest_entry_index),
                _ => return,
            };
            let (session_id, latest_entry_index) = subscribed;
            // Forward Subscribed to client
            let msg = SessionGatewayMessage::Subscribed {
                session_id: session_id.0.clone(),
                latest_entry_index,
            };
            if let Ok(json) = serde_json::to_string(&msg)
                && socket.send(Message::Text(json.into())).await.is_err()
            {
                return;
            }
            info!(session_id = %session_id.0, "session created");
            (subscriber_id, session_id)
        }
        SessionClientMessage::Subscribe { session_id } => {
            let sid = SessionId(session_id.clone());
            let subscription = match state.runtime.session_subscribe(sid.clone(), tx).await {
                Ok(subscription) => subscription,
                Err(err) => {
                    warn!(%err, session_id = %session_id, "failed to subscribe to session");
                    return;
                }
            };
            // Send Subscribed to client. If the session doesn't exist,
            // the kernel emits SubscriberRemoved via P14 defensive cleanup
            // which will arrive through the mpsc channel and be handled
            // in the connection loop below.
            let msg = SessionGatewayMessage::Subscribed {
                session_id: session_id.clone(),
                latest_entry_index: subscription.latest_entry_index,
            };
            if let Ok(json) = serde_json::to_string(&msg)
                && socket.send(Message::Text(json.into())).await.is_err()
            {
                return;
            }
            info!(session_id = %session_id, "subscribed to session");
            (subscription.subscriber_id, sid)
        }
        _ => return, // unreachable given handshake validation above
    };

    // 4. Connection loop
    let heartbeat_period = Duration::from_secs(10);
    let mut heartbeat_interval = interval_at(Instant::now() + heartbeat_period, heartbeat_period);

    loop {
        tokio::select! {
            // Gateway → Client: forward session events
            msg = rx.recv() => {
                match msg {
                    Some(gateway_msg) => {
                        let is_removed = matches!(
                            gateway_msg,
                            SessionGatewayMessage::SubscriberRemoved
                        );
                        if let Ok(json) = serde_json::to_string(&gateway_msg)
                            && socket.send(Message::Text(json.into())).await.is_err()
                        {
                            break;
                        }
                        if is_removed {
                            break; // server removed us
                        }
                    }
                    None => break, // channel closed
                }
            }
            // Client → Gateway: parse and dispatch
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<SessionClientMessage>(&text) {
                            Ok(SessionClientMessage::Append { payload }) => {
                                let _ = state.runtime.session_append_entry(
                                    session_id.clone(), payload,
                                ).await;
                            }
                            Ok(SessionClientMessage::Heartbeat) => {
                                let _ = state.runtime.session_subscriber_heartbeat(
                                    session_id.clone(),
                                    subscriber_id.clone(),
                                ).await;
                            }
                            Ok(SessionClientMessage::Create)
                            | Ok(SessionClientMessage::Subscribe { .. }) => {
                                // Handshake messages after handshake — ignore
                            }
                            Err(_) => {} // skip malformed
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // skip binary/ping/pong
                }
            }
            // Heartbeat tick
            _ = heartbeat_interval.tick() => {
                let _ = state.runtime.session_subscriber_heartbeat(
                    session_id.clone(),
                    subscriber_id.clone(),
                ).await;
            }
        }
    }

    // 5. Cleanup: send Unsubscribed on disconnect
    let _ = state
        .runtime
        .session_unsubscribe(session_id, subscriber_id)
        .await;
}
