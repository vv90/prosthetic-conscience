#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolViolation {
    pub worker_description: String,
    pub message: String,
}

impl ProtocolViolation {
    pub async fn execute(self) {
        tracing::warn!(
            worker = %self.worker_description,
            message = %self.message,
            "protocol violation"
        );
    }
}
