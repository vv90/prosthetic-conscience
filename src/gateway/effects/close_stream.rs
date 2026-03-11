use crate::gateway::channel_registry::{ClientStreamId, StreamHandle};

#[derive(Debug, Clone, PartialEq)]
pub struct CloseStream<SId> {
    pub client_stream_id: SId,
}

impl CloseStream<(ClientStreamId, StreamHandle)> {
    pub async fn execute(self) {
        // Drop the stream handle. If the registry entry was taken in
        // resolve_effects, this is the last sender and the channel closes.
        // No frame is sent — CloseStream is used for pre-dispatch rejections
        // where SendClientError already delivered the error message.
        let (_client_stream_id, _stream_handle) = self.client_stream_id;
    }
}
