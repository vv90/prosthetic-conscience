use std::net::SocketAddr;

use prosthetic_conscience::router::{AppState, router};
use tokio::net::TcpListener;

pub struct TestGateway {
    pub addr: SocketAddr,
}

impl TestGateway {
    pub async fn start() -> Self {
        let state = AppState::new();
        let app = router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr }
    }
}
