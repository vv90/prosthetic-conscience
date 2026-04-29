mod support;

use std::collections::BTreeSet;
use std::time::Duration;

use consensus::engine::{ConsensusEngine, DraftContent};
use consensus::response as response_assembler;
use consensus::types::{ClaimId, ClaimKind, Entry};
use consensus::{app as consensus_app, coordinator as consensus_coordinator};
use prosthetic_conscience::client::gateway_client::GatewayClient;
use prosthetic_conscience::client::tool_loop;
use prosthetic_conscience::client::tools::ToolRegistry;
use prosthetic_conscience::client::tools::current_time::GetCurrentTime;
use prosthetic_conscience::consensus_cli::app::{AppConfig, ConsensusApp};
use prosthetic_conscience::consensus_cli::llm::ConsensusLlm;
use prosthetic_conscience::consensus_cli::seed::join_and_seed_session;
use prosthetic_conscience::consensus_cli::session::SessionClient;
use prosthetic_conscience::consensus_support::fixtures::authentication_deliberation_log;
use prosthetic_conscience::gateway::runtime::GatewayConfig;
use prosthetic_conscience::protocol::Capability;
use prosthetic_conscience::protocol::GatewayToWorker;
use serde_json::json;

use prosthetic_conscience::protocol::SessionGatewayMessage;
use support::client::{SseClient, SseEvent, transcribe};
use support::gateway::TestGateway;
use support::session::MockSessionClient;
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

// ── Session integration tests ───────────────────────────────────────────────

#[tokio::test]
async fn session_create_and_append() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let (mut client, session_id) = MockSessionClient::create(gw.addr).await;
        assert!(!session_id.is_empty());

        client.append(json!({"msg": "hello"})).await;

        let entry = client.recv().await;
        assert_eq!(
            entry,
            Some(SessionGatewayMessage::Entry {
                index: 0,
                payload: json!({"msg": "hello"}),
            }),
        );
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_create_handshake_reports_null_latest_entry_index() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let (_client, session_id, latest_entry_index) =
            MockSessionClient::create_with_handshake(gw.addr).await;

        assert!(!session_id.is_empty());
        assert_eq!(latest_entry_index, None);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_subscribe_receives_notifications() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let (mut client_a, session_id) = MockSessionClient::create(gw.addr).await;
        let mut client_b = MockSessionClient::subscribe(gw.addr, &session_id).await;

        // A appends — both receive
        client_a.append(json!({"data": 1})).await;

        let entry_a = client_a.recv().await;
        let entry_b = client_b.recv().await;
        let expected = Some(SessionGatewayMessage::Entry {
            index: 0,
            payload: json!({"data": 1}),
        });
        assert_eq!(entry_a, expected);
        assert_eq!(entry_b, expected);

        // B appends — both receive
        client_b.append(json!({"data": 2})).await;

        let entry_a2 = client_a.recv().await;
        let entry_b2 = client_b.recv().await;
        let expected2 = Some(SessionGatewayMessage::Entry {
            index: 1,
            payload: json!({"data": 2}),
        });
        assert_eq!(entry_a2, expected2);
        assert_eq!(entry_b2, expected2);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_subscribe_handshake_reports_latest_entry_index() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let (mut writer, session_id) = MockSessionClient::create(gw.addr).await;
        writer.append(json!({"data": 1})).await;
        let _ = writer.recv().await;
        writer.append(json!({"data": 2})).await;
        let _ = writer.recv().await;

        let (_reader, latest_entry_index) =
            MockSessionClient::subscribe_with_handshake(gw.addr, &session_id).await;

        assert_eq!(latest_entry_index, Some(1));
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_subscribe_nonexistent() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let mut client = MockSessionClient::subscribe(gw.addr, "no_such_session").await;

        // P14 defensive cleanup: kernel emits SubscriberRemoved for unknown session
        let msg = client.recv().await;
        assert_eq!(msg, Some(SessionGatewayMessage::SubscriberRemoved));
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_subscriber_timeout() {
    let config = GatewayConfig {
        tick_interval: Duration::from_millis(50),
        subscriber_ttl: 2,
        ..GatewayConfig::default()
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start_with_config(config).await;

        let (mut client, _session_id) = MockSessionClient::create(gw.addr).await;

        // subscriber_ttl=2, tick_interval=50ms → expires in ~100ms.
        // WS handler auto-heartbeat is 10s, so it won't fire in time.
        let msg = client.recv().await;
        assert_eq!(msg, Some(SessionGatewayMessage::SubscriberRemoved));
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_multiple_subscribers() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let (mut client_a, session_id) = MockSessionClient::create(gw.addr).await;
        let mut client_b = MockSessionClient::subscribe(gw.addr, &session_id).await;
        let mut client_c = MockSessionClient::subscribe(gw.addr, &session_id).await;

        client_a.append(json!({"x": 1})).await;

        let expected = Some(SessionGatewayMessage::Entry {
            index: 0,
            payload: json!({"x": 1}),
        });
        assert_eq!(client_a.recv().await, expected);
        assert_eq!(client_b.recv().await, expected);
        assert_eq!(client_c.recv().await, expected);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_disconnect_unsubscribes() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let (mut client_a, session_id) = MockSessionClient::create(gw.addr).await;
        let mut client_b = MockSessionClient::subscribe(gw.addr, &session_id).await;

        // B appends, both receive
        client_b.append(json!({"before": true})).await;
        let _ = client_a.recv().await;
        let _ = client_b.recv().await;

        // B disconnects
        client_b.close().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // A appends — only A should receive
        client_a.append(json!({"after": true})).await;

        let entry = client_a.recv().await;
        assert_eq!(
            entry,
            Some(SessionGatewayMessage::Entry {
                index: 1,
                payload: json!({"after": true}),
            }),
        );
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn session_handshake_timeout() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let gw = TestGateway::start().await;

        let mut client = MockSessionClient::connect_raw(gw.addr).await;

        // Server handshake timeout is 5s. Connection should close.
        let msg = client.recv().await;
        assert_eq!(
            msg, None,
            "expected connection to close after handshake timeout"
        );
    })
    .await
    .expect("test timed out after 10 seconds");
}

#[tokio::test]
async fn consensus_ui_routes_serve_embedded_assets() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let client = reqwest::Client::new();

        let html = client
            .get(format!("http://{}/consensus?session_id=demo", gw.addr))
            .send()
            .await
            .unwrap();
        assert!(html.status().is_success());
        assert!(
            html.headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/html"))
        );
        let html_body = html.text().await.unwrap();
        assert!(html_body.contains("Live Consensus Viewer"));
        assert!(html_body.contains("/consensus-assets/consensus_wasm.js"));

        let js = client
            .get(format!(
                "http://{}/consensus-assets/consensus_wasm.js",
                gw.addr
            ))
            .send()
            .await
            .unwrap();
        assert!(js.status().is_success());
        assert!(
            js.headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("javascript"))
        );
        let js_body = js.text().await.unwrap();
        assert!(js_body.contains("export class ConsensusAppHandle"));
        assert!(js_body.contains("bootstrap("));

        let wasm = client
            .get(format!(
                "http://{}/consensus-assets/consensus_wasm_bg.wasm",
                gw.addr
            ))
            .send()
            .await
            .unwrap();
        assert!(wasm.status().is_success());
        assert_eq!(
            wasm.headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/wasm")
        );
        let bytes = wasm.bytes().await.unwrap();
        assert!(bytes.starts_with(b"\0asm"));
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn consensus_seed_joins_existing_session_and_persists_fixture_entries() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let gw = TestGateway::start().await;
        let log = authentication_deliberation_log();
        let (_subscriber, session_id) = MockSessionClient::create(gw.addr).await;

        let seeded = join_and_seed_session(
            format!("http://{}", gw.addr),
            None,
            session_id.clone(),
            &log.entries,
        )
        .await
        .unwrap();

        assert_eq!(seeded.session_id, session_id);
        assert_eq!(seeded.total_entries, log.entries.len());

        let session = SessionClient::join(
            format!("http://{}", gw.addr),
            None,
            seeded.session_id.clone(),
        )
        .await
        .unwrap();

        let page = session.fetch_entries(0, log.entries.len()).await.unwrap();
        let expected = log
            .entries
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(page.total, expected.len());
        assert_eq!(page.entries, expected);
    })
    .await
    .expect("test timed out after 10 seconds");
}

#[tokio::test]
async fn consensus_bootstrap_from_latest_entry_index_and_fetched_pages_reconstructs_overview() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let gw = TestGateway::start().await;
        let log = authentication_deliberation_log();
        let (_subscriber, session_id) = MockSessionClient::create(gw.addr).await;

        let seeded = join_and_seed_session(
            format!("http://{}", gw.addr),
            None,
            session_id.clone(),
            &log.entries,
        )
        .await
        .unwrap();

        let (_bootstrap_client, latest_entry_index) =
            MockSessionClient::subscribe_with_handshake(gw.addr, &seeded.session_id).await;
        assert_eq!(latest_entry_index, seeded.total_entries.checked_sub(1),);

        let session = SessionClient::join(
            format!("http://{}", gw.addr),
            None,
            seeded.session_id.clone(),
        )
        .await
        .unwrap();
        let page = session
            .fetch_entries(0, seeded.total_entries)
            .await
            .unwrap();

        let transition = consensus_app::init(
            String::from("browser"),
            latest_entry_index,
            consensus_app::ConversationConfig {
                model: String::from("gpt-5.4"),
                max_history: 8,
                max_tokens: 512,
            },
        );
        let mut state = transition.state;
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            consensus_app::Effect::CoordinatorEffect {
                effect: consensus_coordinator::Effect::FetchMissing { from, limit, .. },
            } if *from == 0 && *limit == seeded.total_entries
        ));

        for (index, payload) in page.entries.into_iter().enumerate() {
            let entry: Entry = serde_json::from_value(payload).unwrap();
            state = consensus_app::reduce(
                state,
                consensus_app::Event::CoordinatorEvent {
                    event: consensus_coordinator::Event::Received { index, entry },
                },
            )
            .state;
        }

        let mut expected = ConsensusEngine::new(String::from("browser"));
        for entry in &log.entries {
            expected.append(entry.clone());
        }

        assert_eq!(consensus_app::view(&state).overview, expected.overview());
    })
    .await
    .expect("test timed out after 10 seconds");
}

#[tokio::test]
async fn consensus_seed_fails_for_unknown_session_id() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let log = authentication_deliberation_log();

        let error = join_and_seed_session(
            format!("http://{}", gw.addr),
            None,
            String::from("no-such-session"),
            &log.entries,
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("subscriber removed")
                || error.to_string().contains("unknown session")
                || error.to_string().contains("session"),
            "unexpected error: {error}"
        );
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn consensus_app_join_bootstraps_from_existing_session() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let (mut client, session_id) = MockSessionClient::create(gw.addr).await;

        let payload = serde_json::to_value(Entry::Claim {
            claim_id: ClaimId("item1".into()),
            author: "alice".into(),
            body: "Authentication approach?".into(),
            claim_kind: ClaimKind::Item,
            parent_id: None,
        })
        .unwrap();
        client.append(payload).await;
        let _ = client.recv().await;

        let config = AppConfig {
            gateway_url: format!("http://{}", gw.addr),
            auth_token: None,
            model: "test".into(),
            participant: "assistant".into(),
            max_history: 100,
            debug_tool_trace: false,
        };
        let app = ConsensusApp::join(config, session_id).await.unwrap();
        let overview = app.overview();
        assert_eq!(overview.total_claims, 1);
        assert_eq!(overview.items.len(), 1);
        assert_eq!(overview.items[0].id, ClaimId("item1".into()));
    })
    .await
    .expect("test timed out after 5 seconds");
}

// --- Transcription integration tests ---

#[tokio::test]
async fn transcription_happy_path() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        // Connect a transcription-capable worker.
        let mut worker = MockWorker::connect_with_capabilities(
            gw.addr,
            BTreeSet::from([Capability::Transcription]),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send a transcription request (small fake audio bytes).
        let audio_bytes = b"fake audio data";
        let response_future = transcribe(gw.addr, audio_bytes, "whisper-1");

        // Worker receives job, verifies capability, sends response.
        let (response, _) = tokio::join!(response_future, async {
            let job = worker.recv_job().await;
            let GatewayToWorker::Job {
                capability,
                payload,
                ..
            } = job;
            assert_eq!(
                capability,
                prosthetic_conscience::protocol::Capability::Transcription
            );
            assert!(payload["audio_base64"].is_string());
            assert_eq!(payload["model"], "whisper-1");

            worker.send_chunk(json!({"text": "hello world"})).await;
            worker.send_end().await;
        });

        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["text"], "hello world");
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn transcription_no_worker_returns_error() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        // No transcription worker connected.
        let response = transcribe(gw.addr, b"audio", "whisper-1").await;

        // Should get an error — dispatched via SSE internally, collected as error response.
        assert_eq!(response.status(), 500);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["message"], "no idle worker available");
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn transcription_worker_error() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        let mut worker = MockWorker::connect_with_capabilities(
            gw.addr,
            BTreeSet::from([Capability::Transcription]),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let response_future = transcribe(gw.addr, b"audio", "whisper-1");

        let (response, _) = tokio::join!(response_future, async {
            let _job = worker.recv_job().await;
            worker.send_error("whisper backend unavailable").await;
        });

        assert_eq!(response.status(), 500);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("whisper backend unavailable"),
        );
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn transcription_worker_does_not_receive_chat_jobs() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        // Connect a worker declaring only Transcription capability.
        let _worker = MockWorker::connect_with_capabilities(
            gw.addr,
            BTreeSet::from([Capability::Transcription]),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send a chat request — should fail because no chat-capable worker exists.
        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;
        let events = client.collect_all().await;

        assert_eq!(events.len(), 2, "expected error + done, got: {:?}", events);
        match &events[0] {
            SseEvent::Data(v) => {
                let msg = v["error"]["message"].as_str().unwrap();
                assert_eq!(msg, "no idle worker available");
            }
            other => panic!("expected error event, got: {:?}", other),
        }
        assert_eq!(events[1], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn chat_worker_alongside_transcription_worker() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;

        // Connect a transcription-only worker.
        let _transcription_worker = MockWorker::connect_with_capabilities(
            gw.addr,
            BTreeSet::from([Capability::Transcription]),
        )
        .await;

        // Connect a chat-capable worker.
        let mut chat_worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send a chat request — the chat worker should get it.
        let mut client = SseClient::chat(gw.addr, json!({"model": "test"})).await;

        let job = chat_worker.recv_job().await;
        let GatewayToWorker::Job { payload, .. } = job;
        assert_eq!(payload["model"], "test");

        chat_worker
            .send_chunk(json!({"choices": [{"delta": {"content": "hello"}}]}))
            .await;
        chat_worker.send_end().await;

        let events = client.collect_all().await;
        assert_eq!(events.len(), 2, "expected chunk + done, got: {:?}", events);
        assert_eq!(
            events[0],
            SseEvent::Data(json!({"choices": [{"delta": {"content": "hello"}}]})),
        );
        assert_eq!(events[1], SseEvent::Done);
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn tool_loop_executes_tool_and_re_requests() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = GatewayClient::new(format!("http://{}", gw.addr), None);

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(GetCurrentTime));

        let mut messages = vec![json!({"role": "user", "content": "what time is it?"})];

        // Run the tool loop in a background task.
        let loop_handle = tokio::spawn(async move {
            tool_loop::run(&client, &registry, &mut messages, "test", 10)
                .await
                .map(|msg| (msg, messages))
        });

        // Round 1: worker receives the request and responds with a tool call.
        let job1 = worker.recv_job().await;
        match &job1 {
            GatewayToWorker::Job { payload, .. } => {
                // Verify tools are included in the payload.
                assert!(
                    payload.get("tools").is_some(),
                    "payload should include tools"
                );
                let tools = payload["tools"].as_array().unwrap();
                assert_eq!(tools.len(), 1);
                assert_eq!(tools[0]["function"]["name"], "get_current_time");
            }
        }

        // Worker sends a response with a tool call.
        worker
            .send_chunk(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_abc123",
                            "type": "function",
                            "function": {
                                "name": "get_current_time",
                                "arguments": "{}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .await;
        worker.send_end().await;

        // The tool loop should execute get_current_time and re-request.
        // Worker re-registers after completing the first job.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Round 2: worker receives the follow-up request with tool result.
        let job2 = worker.recv_job().await;
        match &job2 {
            GatewayToWorker::Job { payload, .. } => {
                let msgs = payload["messages"].as_array().unwrap();
                // Should have: user, assistant (with tool_calls), tool result
                assert!(
                    msgs.len() >= 3,
                    "expected at least 3 messages, got {}: {:?}",
                    msgs.len(),
                    msgs
                );

                // Last message should be the tool result.
                let tool_msg = msgs.last().unwrap();
                assert_eq!(tool_msg["role"], "tool");
                assert_eq!(tool_msg["tool_call_id"], "call_abc123");
                // Content should be an ISO 8601 UTC timestamp (e.g. "2026-03-16T11:02:08Z").
                let content = tool_msg["content"].as_str().unwrap();
                assert!(
                    content.ends_with('Z') && content.contains('T'),
                    "tool result should be an ISO 8601 UTC timestamp, got: {content}"
                );
            }
        }

        // Worker sends a final text response.
        worker
            .send_chunk(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": "The current time is 12:00 UTC."
                    },
                    "finish_reason": "stop"
                }]
            }))
            .await;
        worker.send_end().await;

        // The tool loop should return the final message.
        let (msg, messages) = loop_handle
            .await
            .expect("tool loop task panicked")
            .expect("tool loop failed");

        assert_eq!(
            msg.content,
            Some("The current time is 12:00 UTC.".to_owned())
        );
        assert_eq!(
            msg.finish_reason,
            Some(response_assembler::FinishReason::Stop)
        );
        assert!(msg.tool_calls.is_empty());

        // Conversation history should have 4 messages:
        // user, assistant (tool_calls), tool result, assistant (final)
        assert_eq!(
            messages.len(),
            4,
            "expected 4 messages in history, got {}: {:?}",
            messages.len(),
            messages
        );
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn consensus_llm_drafts_claim_after_clarification_turn() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let llm = ConsensusLlm::new(
            format!("http://{}", gw.addr),
            None,
            "test".into(),
            "assistant".into(),
            100,
        );

        let loop_handle = tokio::spawn(async move {
            let mut engine = ConsensusEngine::new(String::from("assistant"));
            let mut history =
                vec![json!({"role": "user", "content": "Draft a proposal to use JWT"})];
            let clarification = llm.run_turn(&mut engine, &mut history).await.unwrap();
            history.push(json!({
                "role": "user",
                "content": "Yes, please prepare that proposal."
            }));
            let draft_reply = llm.run_turn(&mut engine, &mut history).await.unwrap();
            (
                clarification,
                draft_reply,
                history,
                engine.show_drafts().to_vec(),
            )
        });

        // Turn 1: model replies with plain text clarification (no tool calls).
        let job1 = worker.recv_job().await;
        match &job1 {
            GatewayToWorker::Job { payload, .. } => {
                let tools = payload["tools"].as_array().unwrap();
                let tool_names: Vec<&str> = tools
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str())
                    .collect();
                // All tools available on every turn now.
                assert!(tool_names.contains(&"draft_claim"));
                assert!(tool_names.contains(&"impact_analysis"));
                assert!(tool_names.contains(&"show_drafts"));
                assert!(!tool_names.contains(&"submit_drafts"));
                assert!(!tool_names.contains(&"clear_drafts"));
                assert!(!tool_names.contains(&"no_structured_action"));
                assert_eq!(payload["tool_choice"], "auto");
            }
        }

        // Model responds with plain text (no tool calls).
        worker
            .send_chunk(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": "It sounds like you want me to prepare a proposal to use JWT. Should I go ahead and draft that?"
                    },
                    "finish_reason": "stop"
                }]
            }))
            .await;
        worker.send_end().await;

        // Turn 2: after user confirms, model calls draft_claim.
        let job2 = worker.recv_job().await;
        match &job2 {
            GatewayToWorker::Job { payload, .. } => {
                let tools = payload["tools"].as_array().unwrap();
                let tool_names: Vec<&str> = tools
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str())
                    .collect();
                assert!(tool_names.contains(&"draft_claim"));
                assert!(tool_names.contains(&"draft_comment"));
                assert!(tool_names.contains(&"impact_analysis"));
            }
        }

        worker
            .send_chunk(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_draft",
                            "type": "function",
                            "function": {
                                "name": "draft_claim",
                                "arguments": "{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .await;
        worker.send_end().await;

        let (clarification, draft_reply, _history, drafts) = loop_handle.await.unwrap();
        assert_eq!(
            clarification.content,
            Some("It sounds like you want me to prepare a proposal to use JWT. Should I go ahead and draft that?".to_owned())
        );
        assert!(clarification.tool_calls.is_empty());
        assert_eq!(
            draft_reply.content,
            Some("I've prepared a draft for \"Use JWT\". It's still only a local draft, so we can adjust the wording before you submit it.".to_owned())
        );
        assert_eq!(drafts.len(), 1);
        assert!(matches!(
            &drafts[0].entry,
            DraftContent::Claim {
                body,
                claim_kind,
                ..
            } if body == "Use JWT" && *claim_kind == ClaimKind::Proposal
        ));
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn gateway_client_collects_chunks_and_assembles() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        let mut worker = MockWorker::connect(gw.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = GatewayClient::new(format!("http://{}", gw.addr), None);

        // Spawn the client request in a task since it blocks until stream ends.
        let client_handle = tokio::spawn(async move {
            client
                .chat(json!({"model": "test", "messages": [{"role": "user", "content": "hi"}]}))
                .await
        });

        // Worker receives job and sends a multi-chunk response.
        let _job = worker.recv_job().await;
        worker
            .send_chunk(json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}]}))
            .await;
        worker
            .send_chunk(json!({"choices": [{"index": 0, "delta": {"content": "Hello"}, "finish_reason": null}]}))
            .await;
        worker
            .send_chunk(json!({"choices": [{"index": 0, "delta": {"content": " there"}, "finish_reason": null}]}))
            .await;
        worker
            .send_chunk(json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}))
            .await;
        worker.send_end().await;

        let chunks = client_handle
            .await
            .expect("client task panicked")
            .expect("client request failed");

        assert_eq!(chunks.len(), 4);

        let msg = response_assembler::assemble(&chunks).expect("assembly failed");
        assert_eq!(msg.content, Some("Hello there".to_owned()));
        assert_eq!(msg.finish_reason, Some(response_assembler::FinishReason::Stop));
        assert!(msg.tool_calls.is_empty());
    })
    .await
    .expect("test timed out after 5 seconds");
}

#[tokio::test]
async fn gateway_client_returns_error_on_no_workers() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let gw = TestGateway::start().await;
        // No worker connected.

        let client = GatewayClient::new(format!("http://{}", gw.addr), None);
        let result = client
            .chat(json!({"model": "test", "messages": [{"role": "user", "content": "hi"}]}))
            .await;

        // Gateway returns 200 with SSE error (not an HTTP error), so the client
        // gets chunks back. The assembler or caller handles the error content.
        // The key thing is that chat() doesn't panic.
        assert!(result.is_ok());
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

        // Kernel emits SendClientError + SendClientDone.
        // Client receives the error event followed by [DONE].
        assert_eq!(events.len(), 2, "expected error + done, got: {:?}", events);
        match &events[0] {
            SseEvent::Data(v) => {
                let msg = v["error"]["message"].as_str().unwrap();
                assert_eq!(msg, "no idle worker available");
            }
            other => panic!("expected error event, got: {:?}", other),
        }
        assert_eq!(events[1], SseEvent::Done);
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
