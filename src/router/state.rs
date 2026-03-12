use crate::gateway::runtime::{GatewayConfig, GatewayRuntime, RuntimeHandle};

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
        Self::with_config(GatewayConfig::default())
    }

    pub fn with_config(config: GatewayConfig) -> Self {
        Self {
            runtime: GatewayRuntime::spawn(config),
        }
    }
}
