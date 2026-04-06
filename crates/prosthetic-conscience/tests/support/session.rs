use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use prosthetic_conscience::protocol::{SessionClientMessage, SessionGatewayMessage};

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct MockSessionClient {
    ws: WsStream,
}

impl MockSessionClient {
    /// Connect to /v1/sessions, send Create, wait for Subscribed.
    /// Returns (client, session_id).
    pub async fn create(addr: SocketAddr) -> (Self, String) {
        let (client, session_id, _) = Self::create_with_handshake(addr).await;
        (client, session_id)
    }

    /// Connect to /v1/sessions, send Create, wait for Subscribed.
    /// Returns (client, session_id, latest_entry_index).
    pub async fn create_with_handshake(addr: SocketAddr) -> (Self, String, Option<usize>) {
        let url = format!("ws://{}/v1/sessions", addr);
        let (mut ws, _) = connect_async(url)
            .await
            .expect("failed to connect session client");

        let msg = SessionClientMessage::Create;
        let text = serde_json::to_string(&msg).expect("failed to serialize Create");
        ws.send(Message::Text(text))
            .await
            .expect("failed to send Create");

        let mut client = Self { ws };
        match client.recv().await {
            Some(SessionGatewayMessage::Subscribed {
                session_id,
                latest_entry_index,
            }) => (client, session_id, latest_entry_index),
            other => panic!("expected Subscribed, got {:?}", other),
        }
    }

    /// Connect to /v1/sessions, send Subscribe, wait for Subscribed.
    pub async fn subscribe(addr: SocketAddr, session_id: &str) -> Self {
        let (client, _) = Self::subscribe_with_handshake(addr, session_id).await;
        client
    }

    /// Connect to /v1/sessions, send Subscribe, wait for Subscribed.
    /// Returns (client, latest_entry_index).
    pub async fn subscribe_with_handshake(
        addr: SocketAddr,
        session_id: &str,
    ) -> (Self, Option<usize>) {
        let url = format!("ws://{}/v1/sessions", addr);
        let (mut ws, _) = connect_async(url)
            .await
            .expect("failed to connect session client");

        let msg = SessionClientMessage::Subscribe {
            session_id: session_id.to_owned(),
        };
        let text = serde_json::to_string(&msg).expect("failed to serialize Subscribe");
        ws.send(Message::Text(text))
            .await
            .expect("failed to send Subscribe");

        let mut client = Self { ws };
        match client.recv().await {
            Some(SessionGatewayMessage::Subscribed {
                latest_entry_index, ..
            }) => (client, latest_entry_index),
            other => panic!("expected Subscribed, got {:?}", other),
        }
    }

    /// Connect to /v1/sessions without sending a handshake message.
    pub async fn connect_raw(addr: SocketAddr) -> Self {
        let url = format!("ws://{}/v1/sessions", addr);
        let (ws, _) = connect_async(url)
            .await
            .expect("failed to connect session client");
        Self { ws }
    }

    /// Send an Append message.
    pub async fn append(&mut self, payload: Value) {
        let msg = SessionClientMessage::Append { payload };
        let text = serde_json::to_string(&msg).expect("failed to serialize Append");
        self.ws
            .send(Message::Text(text))
            .await
            .expect("failed to send Append");
    }

    /// Send a Heartbeat message.
    #[allow(dead_code)]
    pub async fn send_heartbeat(&mut self) {
        let msg = SessionClientMessage::Heartbeat;
        let text = serde_json::to_string(&msg).expect("failed to serialize Heartbeat");
        self.ws
            .send(Message::Text(text))
            .await
            .expect("failed to send Heartbeat");
    }

    /// Receive and parse the next SessionGatewayMessage. Skips non-text frames.
    /// Returns None when the connection closes.
    pub async fn recv(&mut self) -> Option<SessionGatewayMessage> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    return Some(
                        serde_json::from_str(&text).expect("failed to parse gateway message"),
                    );
                }
                Some(Ok(Message::Close(_))) | None => return None,
                Some(Err(_)) => return None,
                _ => {} // skip binary/ping/pong
            }
        }
    }

    /// Close the WS connection.
    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}
