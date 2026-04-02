use crate::gateway::channel_registry::{ClientStreamId, StreamHandle};
use crate::gateway::relay::StreamFrame;

#[derive(Debug, Clone, PartialEq)]
pub struct SendClientError<SId> {
    pub client_stream_id: SId,
    pub message: String,
}

impl SendClientError<(ClientStreamId, StreamHandle)> {
    pub async fn execute(self) {
        let (_client_stream_id, stream_handle) = self.client_stream_id;
        let frame = StreamFrame::Error {
            message: self.message,
        };
        if stream_handle.send(frame).await.is_err() {
            tracing::debug!("client stream already closed when sending error");
        }
    }
}
