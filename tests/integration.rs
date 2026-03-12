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
async fn concurrent_streams_no_cross_contamination() {
    const NUM_WORKERS: usize = 10;
    const CHUNKS_PER_WORKER: usize = 20;

    tokio::time::timeout(Duration::from_secs(10), async {
        let gw = TestGateway::start().await;

        // Connect workers and wait for registration.
        let mut workers: Vec<MockWorker> = Vec::new();
        for _ in 0..NUM_WORKERS {
            workers.push(MockWorker::connect(gw.addr).await);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Spawn client tasks — each connects, collects all events, returns them.
        let addr = gw.addr;
        let mut client_handles = Vec::new();
        for _ in 0..NUM_WORKERS {
            client_handles.push(tokio::spawn(async move {
                let mut client = SseClient::chat(addr, json!({"model": "test"})).await;
                client.collect_all().await
            }));
        }

        // Spawn worker tasks — each receives a job, sends N tagged chunks, ends.
        let mut worker_handles = Vec::new();
        for mut worker in workers {
            worker_handles.push(tokio::spawn(async move {
                let job = worker.recv_job().await;
                let stream_id = match &job {
                    GatewayToWorker::Job {
                        client_stream_id, ..
                    } => client_stream_id.clone(),
                };

                for i in 0..CHUNKS_PER_WORKER {
                    worker
                        .send_chunk(json!({
                            "choices": [{"delta": {"content": format!("{stream_id}:{i}")}}]
                        }))
                        .await;
                }
                worker.send_end().await;

                stream_id
            }));
        }

        // Await all workers — collect the stream IDs they served.
        let mut worker_stream_ids: Vec<String> = Vec::new();
        for handle in worker_handles {
            worker_stream_ids.push(handle.await.expect("worker task panicked"));
        }

        // Await all clients — collect their events.
        let mut all_client_events: Vec<Vec<SseEvent>> = Vec::new();
        for handle in client_handles {
            all_client_events.push(handle.await.expect("client task panicked"));
        }

        // Validate each client's events.
        let mut seen_stream_ids: Vec<String> = Vec::new();
        for (ci, events) in all_client_events.iter().enumerate() {
            assert_eq!(
                events.len(),
                CHUNKS_PER_WORKER + 1,
                "client {ci} expected {} chunks + done, got: {} events",
                CHUNKS_PER_WORKER,
                events.len()
            );

            // Last event must be [DONE].
            assert_eq!(events[CHUNKS_PER_WORKER], SseEvent::Done);

            // Parse each data chunk: "stream_id:index".
            let mut client_stream_id: Option<String> = None;
            for (i, event) in events[..CHUNKS_PER_WORKER].iter().enumerate() {
                let content = match event {
                    SseEvent::Data(v) => v["choices"][0]["delta"]["content"]
                        .as_str()
                        .expect("missing content"),
                    other => panic!("client {ci} event {i}: expected data, got: {other:?}"),
                };
                let (sid, idx_str) = content
                    .rsplit_once(':')
                    .unwrap_or_else(|| panic!("client {ci} chunk {i} bad format: {content}"));
                let idx: usize = idx_str
                    .parse()
                    .unwrap_or_else(|_| panic!("client {ci} chunk {i} bad index: {idx_str}"));

                // All chunks must carry the same stream ID.
                match &client_stream_id {
                    None => client_stream_id = Some(sid.to_string()),
                    Some(expected) => assert_eq!(
                        sid, expected,
                        "client {ci} chunk {i} cross-contamination: expected {expected}, got {sid}"
                    ),
                }

                // Chunks must arrive in order.
                assert_eq!(
                    idx, i,
                    "client {ci} chunk out of order: expected {i}, got {idx}"
                );
            }

            seen_stream_ids.push(client_stream_id.unwrap());
        }

        // All stream IDs must be distinct (each worker served a different client).
        seen_stream_ids.sort();
        seen_stream_ids.dedup();
        assert_eq!(
            seen_stream_ids.len(),
            NUM_WORKERS,
            "expected {} distinct stream IDs, got {}",
            NUM_WORKERS,
            seen_stream_ids.len()
        );
    })
    .await
    .expect("test timed out after 10 seconds");
}

#[tokio::test]
async fn no_workers_returns_sse_error() {
    tokio::time::timeout(Duration::from_secs(5), async {
        // No worker connected — request goes straight to kernel rejection.
        let gw = TestGateway::start().await;
        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let events = client.collect_all().await;

        // Kernel emits SendClientError + CloseStream (not SendClientDone).
        // CloseStream drops the handle without sending [DONE], so the stream
        // just ends after the error event.
        assert_eq!(events.len(), 1, "expected error only, got: {:?}", events);
        match &events[0] {
            SseEvent::Data(v) => {
                let msg = v["error"]["message"].as_str().unwrap();
                assert_eq!(msg, "no idle worker available");
            }
            other => panic!("expected error event, got: {:?}", other),
        }
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn worker_error_sends_error_and_done() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let _job = worker.recv_job().await;

        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "partial"}}]}))
            .await;
        worker.send_error("out of memory").await;

        let events = client.collect_all().await;

        // Relay sends StreamFrame::Error directly, then returns WorkerError.
        // The handler calls assignment_failed, which makes the kernel emit
        // SendClientError again + SendClientDone. So the client sees:
        // chunk, error (relay), error (kernel), done.
        assert_eq!(
            events.len(),
            4,
            "expected chunk + 2 errors + done, got: {:?}",
            events
        );
        assert_eq!(
            events[0],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "partial"}}]})),
        );
        match &events[1] {
            SseEvent::Data(v) => {
                assert_eq!(v["error"]["message"].as_str().unwrap(), "out of memory");
            }
            other => panic!("expected error event, got: {:?}", other),
        }
        assert_eq!(events[3], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn worker_re_registration_handles_second_job() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // First job
        let mut client1 = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let _job1 = worker.recv_job().await;
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "first"}}]}))
            .await;
        worker.send_end().await;
        let events1 = client1.collect_all().await;
        assert_eq!(events1.len(), 2); // chunk + done

        // Worker re-registers internally after WorkerEnd.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Second job on same worker connection
        let mut client2 = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let _job2 = worker.recv_job().await;
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "second"}}]}))
            .await;
        worker.send_end().await;
        let events2 = client2.collect_all().await;
        assert_eq!(events2.len(), 2); // chunk + done

        assert_eq!(
            events2[0],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "second"}}]})),
        );
        assert_eq!(events2[1], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn completed_streams_drain_from_state() {
    let config = GatewayConfig {
        stream_ttl: 3,
        worker_ttl: 3,
        tick_interval: Duration::from_millis(50),
        // Use default 10s heartbeat — job 3 needs to actually time out,
        // so heartbeats must not reset the deadline before 3 ticks (~150ms).
        ..GatewayConfig::default()
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start_with_config(config).await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Job 1: success path
        let mut c1 = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let _j1 = worker.recv_job().await;
        worker
            .send_chunk(json!({"choices": [{"delta": {"content": "ok"}}]}))
            .await;
        worker.send_end().await;
        c1.collect_all().await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Job 2: worker error path
        let mut c2 = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let _j2 = worker.recv_job().await;
        worker.send_error("fail").await;
        c2.collect_all().await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Job 3: timeout path — worker accepts but never responds
        let mut c3 = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let _j3 = worker.recv_job().await;
        // Don't send anything — let stream_ttl expire (~150ms)
        c3.collect_all().await;

        // After timeout, relay sees ClientGone, worker re-registers.
        // Give time for re-registration + timeout processing.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Disconnect worker, then wait for kernel worker TTL expiry.
        worker.disconnect().await;
        // worker_ttl=3, tick_interval=50ms → expires in ~150ms
        tokio::time::sleep(Duration::from_millis(300)).await;

        let snapshot = gw.runtime.query_state().await.unwrap();
        assert_eq!(snapshot.active_streams, 0, "leaked active streams");
        assert_eq!(
            snapshot.stream_registry_count, 0,
            "leaked stream registry entries"
        );
        assert_eq!(
            snapshot.available_workers, 0,
            "leaked kernel worker entries"
        );
        assert_eq!(
            snapshot.worker_registry_count, 0,
            "leaked worker registry entries"
        );
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
