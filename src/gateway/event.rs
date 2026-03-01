use crate::gateway::{
    job::Job,
    worker_table::{WorkerCapabilities, WorkerId},
};

pub enum Event {
    WorkerConnected {
        worker_id: WorkerId,
        capabilities: WorkerCapabilities,
        instant: std::time::Instant,
    },
    WorkerHeartbeatReceived {
        worker_id: WorkerId,
        instant: std::time::Instant,
    },
    JobRequested {
        job: Job,
    },
}
