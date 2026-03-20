use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum ViolationSource {
    Worker(String),
    Stream(String),
    Session(String),
}

impl fmt::Display for ViolationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViolationSource::Worker(id) => write!(f, "worker {}", id),
            ViolationSource::Stream(id) => write!(f, "stream {}", id),
            ViolationSource::Session(id) => write!(f, "session {}", id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolViolation {
    pub source: ViolationSource,
    pub message: String,
}

impl ProtocolViolation {
    pub async fn execute(self) {
        tracing::warn!(
            source = %self.source,
            message = %self.message,
            "protocol violation"
        );
    }
}
