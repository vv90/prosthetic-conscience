use crate::gateway::channel_registry::{ClientStreamId, StreamHandle};
use crate::gateway::relay::StreamFrame;

#[derive(Debug, Clone, PartialEq)]
pub struct SendClientDone<SId> {
    pub client_stream_id: SId,
}

impl SendClientDone<(ClientStreamId, StreamHandle)> {
    pub async fn execute(self) {
        let (_client_stream_id, stream_handle) = self.client_stream_id;
        if stream_handle.send(StreamFrame::Done).await.is_err() {
            tracing::debug!("client stream already closed when sending done");
        }
        // stream_handle dropped here — if registry entry was taken,
        // this is the last sender and the channel closes.
    }
}
