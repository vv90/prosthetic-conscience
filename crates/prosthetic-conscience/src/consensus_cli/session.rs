use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::protocol::{SessionClientMessage, SessionGatewayMessage};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const EVENT_BUFFER: usize = 256;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("invalid gateway URL: {0}")]
    InvalidGatewayUrl(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("websocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("gateway returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("session command channel closed")]
    CommandChannelClosed,
    #[error("session event channel closed")]
    EventChannelClosed,
    #[error("session disconnected: {0}")]
    Disconnected(String),
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Entry { index: usize, payload: Value },
    Disconnected { reason: String },
    Reconnected,
    Warning(String),
}

#[derive(Debug)]
enum SessionCommand {
    Append {
        payload: Value,
        reply_tx: oneshot::Sender<Result<(), SessionError>>,
    },
}

#[derive(Debug, Clone)]
pub struct SessionEntriesPage {
    pub start_index: usize,
    pub entries: Vec<Value>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
struct SessionEntriesResponse {
    entries: Vec<Value>,
    total: usize,
}

pub struct SessionClient {
    http: reqwest::Client,
    base_url: String,
    auth_token: Option<String>,
    session_id: String,
    cmd_tx: mpsc::Sender<SessionCommand>,
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionClient {
    pub async fn create(
        base_url: String,
        auth_token: Option<String>,
    ) -> Result<Self, SessionError> {
        let base_url = normalize_base_url(&base_url);
        let ws_url = session_ws_url(&base_url)?;
        let (ws, session_id, _) =
            connect_with_handshake(&ws_url, &auth_token, SessionClientMessage::Create).await?;
        Self::from_connected(base_url, auth_token, session_id, ws)
    }

    pub async fn join(
        base_url: String,
        auth_token: Option<String>,
        session_id: String,
    ) -> Result<Self, SessionError> {
        let base_url = normalize_base_url(&base_url);
        let ws_url = session_ws_url(&base_url)?;
        let handshake = SessionClientMessage::Subscribe {
            session_id: session_id.clone(),
        };
        let (ws, _, _) = connect_with_handshake(&ws_url, &auth_token, handshake).await?;
        Self::from_connected(base_url, auth_token, session_id, ws)
    }

    fn from_connected(
        base_url: String,
        auth_token: Option<String>,
        session_id: String,
        ws: WsStream,
    ) -> Result<Self, SessionError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER);
        let ws_url = session_ws_url(&base_url)?;

        tokio::spawn(run_session_task(
            ws,
            ws_url,
            auth_token.clone(),
            session_id.clone(),
            cmd_rx,
            event_tx,
        ));

        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
            auth_token,
            session_id,
            cmd_tx,
            event_rx,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn append_json(&self, payload: Value) -> Result<(), SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCommand::Append { payload, reply_tx })
            .await
            .map_err(|_| SessionError::CommandChannelClosed)?;
        reply_rx
            .await
            .map_err(|_| SessionError::CommandChannelClosed)?
    }

    pub async fn fetch_entries(
        &self,
        start_index: usize,
        limit: usize,
    ) -> Result<SessionEntriesPage, SessionError> {
        let url = format!(
            "{}/v1/sessions/{}/entries?from={start_index}&limit={limit}",
            self.base_url, self.session_id
        );

        let mut request = self.http.get(&url);
        if let Some(token) = &self.auth_token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        let response = request.send().await?;
        let status = response.status();
        if status != StatusCode::OK {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            return Err(SessionError::Status {
                status: status.as_u16(),
                body,
            });
        }

        let page: SessionEntriesResponse = response.json().await?;
        Ok(SessionEntriesPage {
            start_index,
            entries: page.entries,
            total: page.total,
        })
    }

    pub async fn next_event(&mut self) -> Result<SessionEvent, SessionError> {
        self.event_rx
            .recv()
            .await
            .ok_or(SessionError::EventChannelClosed)
    }

    pub fn try_next_event(&mut self) -> Option<SessionEvent> {
        self.event_rx.try_recv().ok()
    }
}

async fn run_session_task(
    initial_ws: WsStream,
    ws_url: String,
    auth_token: Option<String>,
    session_id: String,
    mut cmd_rx: mpsc::Receiver<SessionCommand>,
    event_tx: mpsc::Sender<SessionEvent>,
) {
    let mut current_ws = Some(initial_ws);
    let mut backoff = BACKOFF_INITIAL;
    let mut disconnected = false;

    loop {
        let ws = match current_ws.take() {
            Some(ws) => ws,
            None => match reconnect(&ws_url, &auth_token, &session_id).await {
                Ok(ws) => {
                    backoff = BACKOFF_INITIAL;
                    if event_tx.send(SessionEvent::Reconnected).await.is_err() {
                        return;
                    }
                    ws
                }
                Err(error) => {
                    let reason = error.to_string();
                    if !disconnected {
                        if event_tx
                            .send(SessionEvent::Disconnected {
                                reason: reason.clone(),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        disconnected = true;
                    }

                    let sleep = tokio::time::sleep(backoff);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            maybe_cmd = cmd_rx.recv() => {
                                match maybe_cmd {
                                    Some(SessionCommand::Append { reply_tx, .. }) => {
                                        let _ = reply_tx.send(Err(SessionError::Disconnected(reason.clone())));
                                    }
                                    None => return,
                                }
                            }
                        }
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            },
        };

        match connected_loop(ws, &mut cmd_rx, &event_tx).await {
            Ok(()) => return,
            Err(reason) => {
                if event_tx
                    .send(SessionEvent::Disconnected {
                        reason: reason.clone(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                disconnected = true;
                current_ws = None;
            }
        }
    }
}

async fn connected_loop(
    ws: WsStream,
    cmd_rx: &mut mpsc::Receiver<SessionCommand>,
    event_tx: &mpsc::Sender<SessionEvent>,
) -> Result<(), String> {
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => {
                match maybe_cmd {
                    Some(SessionCommand::Append { payload, reply_tx }) => {
                        let msg = SessionClientMessage::Append { payload };
                        let text = serde_json::to_string(&msg)
                            .map_err(|e| format!("failed to serialize append message: {e}"))?;
                        match sink.send(Message::Text(text)).await {
                            Ok(()) => {
                                let _ = reply_tx.send(Ok(()));
                            }
                            Err(e) => {
                                let reason = format!("failed to send append over websocket: {e}");
                                let _ = reply_tx.send(Err(SessionError::Disconnected(reason.clone())));
                                return Err(reason);
                            }
                        }
                    }
                    None => return Ok(()),
                }
            }
            maybe_msg = stream.next() => {
                let msg = match maybe_msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => return Err(format!("websocket read error: {e}")),
                    None => return Err(String::from("gateway closed session websocket")),
                };

                match msg {
                    Message::Text(text) => {
                        let gateway_msg: SessionGatewayMessage = serde_json::from_str(&text)
                            .map_err(|e| format!("failed to parse session message: {e}"))?;
                        match gateway_msg {
                            SessionGatewayMessage::Entry { index, payload } => {
                                if event_tx.send(SessionEvent::Entry { index, payload }).await.is_err() {
                                    return Ok(());
                                }
                            }
                            SessionGatewayMessage::Error { message } => {
                                if event_tx.send(SessionEvent::Warning(message)).await.is_err() {
                                    return Ok(());
                                }
                            }
                            SessionGatewayMessage::SubscriberRemoved => {
                                return Err(String::from("subscriber removed by gateway"));
                            }
                            SessionGatewayMessage::Subscribed { .. } => {}
                        }
                    }
                    Message::Close(_) => return Err(String::from("gateway sent close frame")),
                    _ => {}
                }
            }
        }
    }
}

async fn reconnect(
    ws_url: &str,
    auth_token: &Option<String>,
    session_id: &str,
) -> Result<WsStream, SessionError> {
    let handshake = SessionClientMessage::Subscribe {
        session_id: session_id.to_owned(),
    };
    let (ws, returned_session_id, _) =
        connect_with_handshake(ws_url, auth_token, handshake).await?;
    if returned_session_id != session_id {
        return Err(SessionError::Protocol(format!(
            "reconnected to unexpected session {returned_session_id}"
        )));
    }
    Ok(ws)
}

async fn connect_with_handshake(
    ws_url: &str,
    auth_token: &Option<String>,
    handshake: SessionClientMessage,
) -> Result<(WsStream, String, Option<usize>), SessionError> {
    let request = build_ws_request(ws_url, auth_token)?;
    let (mut ws, _) = connect_async(request).await.map_err(Box::new)?;

    let text = serde_json::to_string(&handshake)
        .map_err(|e| SessionError::Protocol(format!("failed to serialize handshake: {e}")))?;
    ws.send(Message::Text(text)).await.map_err(Box::new)?;

    loop {
        let msg = ws
            .next()
            .await
            .ok_or_else(|| {
                SessionError::Protocol(String::from("session websocket closed during handshake"))
            })?
            .map_err(Box::new)?;

        match msg {
            Message::Text(text) => {
                let gateway_msg: SessionGatewayMessage =
                    serde_json::from_str(&text).map_err(|e| {
                        SessionError::Protocol(format!("failed to parse handshake response: {e}"))
                    })?;
                match gateway_msg {
                    SessionGatewayMessage::Subscribed {
                        session_id,
                        latest_entry_index,
                    } => {
                        return Ok((ws, session_id, latest_entry_index));
                    }
                    SessionGatewayMessage::Error { message } => {
                        return Err(SessionError::Protocol(message));
                    }
                    SessionGatewayMessage::SubscriberRemoved => {
                        return Err(SessionError::Protocol(String::from(
                            "subscriber removed during handshake",
                        )));
                    }
                    SessionGatewayMessage::Entry { .. } => {}
                }
            }
            Message::Close(_) => {
                return Err(SessionError::Protocol(String::from(
                    "gateway closed session websocket during handshake",
                )));
            }
            _ => {}
        }
    }
}

fn build_ws_request(
    ws_url: &str,
    auth_token: &Option<String>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, SessionError> {
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| SessionError::InvalidGatewayUrl(e.to_string()))?;

    if let Some(token) = auth_token {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|e| SessionError::InvalidGatewayUrl(format!("invalid auth token: {e}")))?,
        );
    }

    Ok(request)
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_owned()
}

fn session_ws_url(base_url: &str) -> Result<String, SessionError> {
    if let Some(rest) = base_url.strip_prefix("http://") {
        Ok(format!("ws://{rest}/v1/sessions"))
    } else if let Some(rest) = base_url.strip_prefix("https://") {
        Ok(format!("wss://{rest}/v1/sessions"))
    } else {
        Err(SessionError::InvalidGatewayUrl(format!(
            "expected http:// or https:// base URL, got {base_url}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_url() {
        assert_eq!(
            normalize_base_url("http://127.0.0.1:3000/"),
            "http://127.0.0.1:3000"
        );
    }

    #[test]
    fn builds_ws_url_from_http_base() {
        assert_eq!(
            session_ws_url("http://127.0.0.1:3000").unwrap(),
            "ws://127.0.0.1:3000/v1/sessions"
        );
        assert_eq!(
            session_ws_url("https://example.com").unwrap(),
            "wss://example.com/v1/sessions"
        );
    }
}
