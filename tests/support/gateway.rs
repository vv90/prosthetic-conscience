use std::net::SocketAddr;

use prosthetic_conscience::gateway::runtime::{GatewayConfig, RuntimeHandle};
use prosthetic_conscience::router::{AppState, router};
use tokio::net::TcpListener;

pub struct TestGateway {
    pub addr: SocketAddr,
    #[allow(dead_code)] // used by upcoming leak detection tests
    pub runtime: RuntimeHandle,
}

impl TestGateway {
    pub async fn start() -> Self {
        Self::start_with_config(GatewayConfig::default()).await
    }

    pub async fn start_with_config(config: GatewayConfig) -> Self {
        let state = AppState::with_config(config);
        let runtime = state.runtime.clone();
        let app = router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, runtime }
    }
}
