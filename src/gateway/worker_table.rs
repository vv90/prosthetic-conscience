use std::collections::HashMap;

use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkerId(pub Uuid);

pub struct WorkerCapabilities {
    pub model: String,
}

pub struct WorkerRecord {
    pub capabilities: WorkerCapabilities,
    pub last_heartbeat_received_at: std::time::Instant,
}
pub struct WorkerTable(HashMap<WorkerId, WorkerRecord>);

impl WorkerTable {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn get_worker(&self, worker_id: &WorkerId) -> Option<&WorkerRecord> {
        let WorkerTable(workers_map) = self;
        workers_map.get(worker_id)
    }

    pub fn with_worker_added(self, worker_id: WorkerId, record: WorkerRecord) -> Self {
        let WorkerTable(mut workers_map) = self;
        workers_map.insert(worker_id, record);
        Self(workers_map)
    }

    pub fn with_worker_removed(self, worker_id: &WorkerId) -> Self {
        let WorkerTable(mut workers_map) = self;
        workers_map.remove(worker_id);
        Self(workers_map)
    }

    pub fn with_worker_heartbeat_updated(
        self,
        worker_id: &WorkerId,
        instant: std::time::Instant,
    ) -> Self {
        let WorkerTable(mut workers_map) = self;
        if let Some(record) = workers_map.get_mut(worker_id) {
            record.last_heartbeat_received_at = instant;
        }
        Self(workers_map)
    }
}
