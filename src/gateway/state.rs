use crate::gateway::worker_table::WorkerTable;

pub struct State {
    pub worker_table: WorkerTable,
}

impl State {
    pub fn new() -> Self {
        Self {
            worker_table: WorkerTable::new(),
        }
    }
}
