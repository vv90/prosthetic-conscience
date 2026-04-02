use std::collections::BTreeSet;
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use prosthetic_conscience::protocol::Capability;
use prosthetic_conscience::protocol::{GatewayToWorker, WorkerMessage};

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct MockWorker {
    ws: WsStream,
}

impl MockWorker {
    /// Connect a mock worker declaring the given capabilities.
    pub async fn connect_with_capabilities(
        addr: SocketAddr,
        capabilities: BTreeSet<Capability>,
    ) -> Self {
        let caps_str: String = capabilities
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let url = format!("ws://{}/ws/worker?capabilities={}", addr, caps_str);
        let (ws, _response) = connect_async(url)
            .await
            .expect("failed to connect mock worker");
        Self { ws }
    }

    /// Connect a mock worker declaring only the `Chat` capability (default).
    pub async fn connect(addr: SocketAddr) -> Self {
        Self::connect_with_capabilities(addr, BTreeSet::from([Capability::Chat])).await
    }

    /// Read the next text message from the gateway and deserialize it as a
    /// `GatewayToWorker` message. Skips non-text frames (ping/pong/binary).
    pub async fn recv_job(&mut self) -> GatewayToWorker {
        loop {
            let msg = self
                .ws
                .next()
                .await
                .expect("websocket stream ended unexpectedly")
                .expect("websocket read error");

            if let Message::Text(text) = msg {
                return serde_json::from_str(&text).expect("failed to parse gateway message");
            }
            // Skip ping/pong/binary/close frames
        }
    }

    pub async fn send_chunk(&mut self, data: Value) {
        let msg = WorkerMessage::Chunk { data };
        let text = serde_json::to_string(&msg).unwrap();
        self.ws.send(Message::Text(text)).await.unwrap();
    }

    pub async fn send_end(&mut self) {
        let msg = WorkerMessage::End;
        let text = serde_json::to_string(&msg).unwrap();
        self.ws.send(Message::Text(text)).await.unwrap();
    }

    pub async fn send_error(&mut self, message: &str) {
        let msg = WorkerMessage::Error {
            message: message.to_owned(),
        };
        let text = serde_json::to_string(&msg).unwrap();
        self.ws.send(Message::Text(text)).await.unwrap();
    }

    pub async fn disconnect(mut self) {
        let _ = self.ws.close(None).await;
    }
}
