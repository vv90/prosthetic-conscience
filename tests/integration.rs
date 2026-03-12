mod support;

use std::time::Duration;

use prosthetic_conscience::gateway::runtime::GatewayConfig;
use prosthetic_conscience::protocol::GatewayToWorker;
use serde_json::json;

use support::client::{SseClient, SseEvent};
use support::gateway::TestGateway;
use support::worker::MockWorker;

#[tokio::test]
async fn happy_path_streams_chunks_and_done() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;

        // Give the worker handler time to register with the runtime.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;

        // Worker receives the dispatched job.
        let job = worker.recv_job().await;
        match &job {
            GatewayToWorker::Job { payload, .. } => {
                assert_eq!(payload["model"], "test");
            }
        }

        // Worker streams two chunks and an end signal.
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "hello"}}]}))
            .await;
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": " world"}}]}))
            .await;
        worker.send_end().await;

        // Client receives the two chunks followed by [DONE].
        let events = client.collect_all().await;
        assert_eq!(events.len(), 3, "expected 3 events, got: {:?}", events);
        assert_eq!(
            events[0],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "hello"}}]})),
        );
        assert_eq!(
            events[1],
            SseEvent::Data(json!({"choices": [{"delta": {"content": " world"}}]})),
        );
        assert_eq!(events[2], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn stream_timeout_sends_error_and_done() {
    let config = GatewayConfig {
        stream_ttl: 3,
        tick_interval: Duration::from_millis(50),
        ..GatewayConfig::default()
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start_with_config(config).await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = SseClient::chat(
            gw.addr,
            json!({"model": "test", "messages": [{"role": "user", "content": "hello"}]}),
        )
        .await;

        // Worker accepts the job but never responds.
        let _job = worker.recv_job().await;

        // Client should receive timeout error + done after ~150ms (3 ticks * 50ms).
        let events = client.collect_all().await;

        assert_eq!(events.len(), 2, "expected error + done, got: {:?}", events);

        match &events[0] {
            SseEvent::Data(v) => {
                let msg = v["error"]["message"].as_str().unwrap();
                assert_eq!(msg, "stream timed out");
            }
            other => panic!("expected error event, got: {:?}", other),
        }
        assert_eq!(events[1], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn worker_disconnect_mid_stream_sends_error_and_done() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;

        // Worker receives job, sends one chunk, then disconnects.
        let _job = worker.recv_job().await;
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "partial"}}]}))
            .await;
        worker.disconnect().await;

        // Client should receive: the chunk, an error, and [DONE].
        let events = client.collect_all().await;
        assert!(
            events.len() >= 2,
            "expected at least error + done, got: {:?}",
            events
        );

        // First event is the chunk that arrived before disconnect.
        assert_eq!(
            events[0],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "partial"}}]})),
        );

        // There should be an error event containing the disconnect message.
        let has_error = events
            .iter()
            .any(|e| matches!(e, SseEvent::Data(v) if v.get("error").is_some()));
        assert!(has_error, "expected an error event, got: {:?}", events);

        // Last event should be [DONE].
        assert_eq!(events.last().unwrap(), &SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn long_running_stream_survives_with_heartbeats() {
    let config = GatewayConfig {
        tick_interval: Duration::from_millis(50),
        stream_ttl: 3, // would expire at ~150ms without heartbeats
        stream_heartbeat_interval: Duration::from_millis(100), // resets deadline before expiry
        ..GatewayConfig::default()
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start_with_config(config).await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;

        let _job = worker.recv_job().await;

        // Send first chunk immediately.
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "first"}}]}))
            .await;

        // Sleep well past the original TTL (150ms). Heartbeats keep the stream alive.
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Stream is still alive — send second chunk and end.
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "second"}}]}))
            .await;
        worker.send_end().await;

        let events = client.collect_all().await;

        // Both chunks arrived, no timeout error.
        assert_eq!(
            events.len(),
            3,
            "expected 2 chunks + done, got: {:?}",
            events
        );
        assert_eq!(
            events[0],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "first"}}]})),
        );
        assert_eq!(
            events[1],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "second"}}]})),
        );
        assert_eq!(events[2], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}
