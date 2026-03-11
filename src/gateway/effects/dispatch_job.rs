use serde_json::Value;
use tokio::sync::oneshot;

use crate::gateway::channel_registry::{ClientStreamId, StreamHandle, WorkerJob};
use crate::gateway::runtime::RuntimeHandle;

#[derive(Debug, PartialEq)]
pub struct DispatchJob<WId, SId> {
    pub worker_id: WId,
    pub client_stream_id: SId,
    pub payload: Value,
}

impl DispatchJob<oneshot::Sender<WorkerJob>, (ClientStreamId, StreamHandle)> {
    pub async fn execute(self, runtime: &RuntimeHandle) {
        let (client_stream_id, client_tx) = self.client_stream_id;
        let job = WorkerJob {
            client_stream_id: client_stream_id.clone(),
            payload: self.payload,
            client_tx,
        };
        if self.worker_id.send(job).is_err() {
            tracing::warn!("worker channel closed before job dispatch");
            let _ = runtime
                .assignment_failed(
                    client_stream_id,
                    String::from("worker channel closed before job dispatch"),
                )
                .await;
        }
    }
}
