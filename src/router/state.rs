use std::time::Duration;

use crate::gateway::runtime::{GatewayRuntime, RuntimeHandle};

#[derive(Clone)]
pub struct AppState {
    pub runtime: RuntimeHandle,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            runtime: GatewayRuntime::spawn(Duration::from_secs(1)),
        }
    }
}
