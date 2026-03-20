use im::{HashMap, HashSet, OrdMap};

use crate::gateway::session;
pub use crate::protocol::Capability;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub String);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerEntry {
    pub deadline: u64,
    pub capabilities: HashSet<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayState<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash> {
    pub tick: u64,
    pub worker_ttl: u64,
    pub stream_ttl: u64,
    pub available: OrdMap<WId, WorkerEntry>,
    pub active_streams: HashMap<SId, u64>,
    pub sessions: HashMap<SessionId, session::State<SId>>,
}

impl<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash> GatewayState<WId, SId> {
    pub fn new(worker_ttl: u64, stream_ttl: u64) -> Self {
        Self {
            tick: 0,
            worker_ttl,
            stream_ttl,
            available: OrdMap::new(),
            active_streams: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

impl<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash> Default for GatewayState<WId, SId> {
    fn default() -> Self {
        Self::new(60, 30)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event<WId, SId> {
    HttpChatRequested {
        client_stream_id: SId,
        payload: Value,
        stream: bool,
        required_capability: Capability,
    },
    WorkerRegistered {
        worker_id: WId,
        capabilities: HashSet<Capability>,
    },
    AssignmentCleared {
        client_stream_id: SId,
    },
    AssignmentFailed {
        client_stream_id: SId,
        message: String,
    },
    WorkerHeartbeat {
        worker_id: WId,
    },
    StreamHeartbeat {
        client_stream_id: SId,
    },
    Tick,
    SessionEvent {
        session_id: SessionId,
        event: session::Event<SId>,
    },
}

use super::effects::{
    dispatch_job::DispatchJob, protocol_violation::ProtocolViolation,
    send_client_done::SendClientDone, send_client_error::SendClientError,
};

#[derive(Debug, PartialEq)]
pub enum Effect<WId, SId> {
    DispatchJob(DispatchJob<WId, SId>),
    SendClientError(SendClientError<SId>),
    SendClientDone(SendClientDone<SId>),
    SessionEffect(session::Effect<SId>),
    ProtocolViolation(ProtocolViolation),
}

pub struct Transition<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash> {
    pub state: GatewayState<WId, SId>,
    pub effects: Vec<Effect<WId, SId>>,
}

// impl<WId, SId> Transition<WId, SId> {
//     fn map(self, )
// }

pub fn reduce<WId, SId>(
    state: GatewayState<WId, SId>,
    event: Event<WId, SId>,
) -> Transition<WId, SId>
where
    WId: Clone + Ord + std::fmt::Display,
    SId: Clone + Eq + std::hash::Hash + std::fmt::Display,
{
    match event {
        Event::HttpChatRequested {
            client_stream_id,
            stream,
            payload,
            required_capability,
        } => {
            if !stream {
                return Transition {
                    state,
                    effects: vec![
                        Effect::SendClientError(SendClientError {
                            client_stream_id: client_stream_id.clone(),
                            message: String::from("stream=true is required in this phase"),
                        }),
                        Effect::SendClientDone(SendClientDone { client_stream_id }),
                    ],
                };
            }

            if state.active_streams.contains_key(&client_stream_id) {
                return Transition {
                    state,
                    effects: vec![
                        Effect::SendClientError(SendClientError {
                            client_stream_id: client_stream_id.clone(),
                            message: String::from("stream already has an active assignment"),
                        }),
                        Effect::SendClientDone(SendClientDone { client_stream_id }),
                    ],
                };
            }

            match first_capable_worker_id(&state, &required_capability) {
                Some(worker_id) => {
                    let deadline = state.tick + state.stream_ttl;
                    Transition {
                        state: GatewayState {
                            available: state.available.without(&worker_id),
                            active_streams: state
                                .active_streams
                                .update(client_stream_id.clone(), deadline),
                            ..state
                        },
                        effects: vec![Effect::DispatchJob(DispatchJob {
                            worker_id,
                            client_stream_id,
                            capability: required_capability,
                            payload,
                        })],
                    }
                }
                None => Transition {
                    state,
                    effects: vec![
                        Effect::SendClientError(SendClientError {
                            client_stream_id: client_stream_id.clone(),
                            message: String::from("no idle worker available"),
                        }),
                        Effect::SendClientDone(SendClientDone { client_stream_id }),
                    ],
                },
            }
        }
        Event::WorkerRegistered {
            worker_id,
            capabilities,
        } => {
            if state.available.contains_key(&worker_id) {
                return Transition {
                    state,
                    effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                        worker_description: worker_id.to_string(),
                        message: String::from("duplicate worker registration"),
                    })],
                };
            }
            let deadline = state.tick + state.worker_ttl;
            Transition {
                state: GatewayState {
                    available: state.available.update(
                        worker_id,
                        WorkerEntry {
                            deadline,
                            capabilities,
                        },
                    ),
                    ..state
                },
                effects: Vec::new(),
            }
        }
        Event::AssignmentCleared { client_stream_id } => {
            if !state.active_streams.contains_key(&client_stream_id) {
                return Transition {
                    state,
                    effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                        worker_description: String::from("unknown"),
                        message: format!(
                            "assignment cleared for unknown stream {}",
                            client_stream_id
                        ),
                    })],
                };
            }

            Transition {
                state: GatewayState {
                    active_streams: state.active_streams.without(&client_stream_id),
                    ..state
                },
                effects: vec![Effect::SendClientDone(SendClientDone { client_stream_id })],
            }
        }
        Event::AssignmentFailed {
            client_stream_id,
            message,
        } => {
            if !state.active_streams.contains_key(&client_stream_id) {
                return Transition {
                    state,
                    effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                        worker_description: String::from("unknown"),
                        message: format!(
                            "assignment failed for unknown stream {}",
                            client_stream_id
                        ),
                    })],
                };
            }

            Transition {
                state: GatewayState {
                    active_streams: state.active_streams.without(&client_stream_id),
                    ..state
                },
                effects: vec![
                    Effect::SendClientError(SendClientError {
                        client_stream_id: client_stream_id.clone(),
                        message,
                    }),
                    Effect::SendClientDone(SendClientDone { client_stream_id }),
                ],
            }
        }
        Event::WorkerHeartbeat { worker_id } => match state.available.extract(&worker_id) {
            Some((entry, available)) => Transition {
                state: GatewayState {
                    available: available.update(
                        worker_id,
                        WorkerEntry {
                            deadline: state.tick + state.worker_ttl,
                            ..entry
                        },
                    ),
                    ..state
                },
                effects: Vec::new(),
            },
            None => Transition {
                state,
                effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                    worker_description: worker_id.to_string(),
                    message: String::from("heartbeat from unknown worker"),
                })],
            },
        },
        Event::StreamHeartbeat { client_stream_id } => {
            if !state.active_streams.contains_key(&client_stream_id) {
                return Transition {
                    state,
                    effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                        worker_description: String::from("unknown"),
                        message: format!("heartbeat for unknown stream {}", client_stream_id),
                    })],
                };
            }

            let new_deadline = state.tick + state.stream_ttl;
            Transition {
                state: GatewayState {
                    active_streams: state.active_streams.update(client_stream_id, new_deadline),
                    ..state
                },
                effects: Vec::new(),
            }
        }
        Event::Tick => {
            let tick = state.tick + 1;

            let available: OrdMap<WId, WorkerEntry> = state
                .available
                .into_iter()
                .filter(|(_, entry)| entry.deadline > tick)
                .collect();

            let mut kept: HashMap<SId, u64> = HashMap::new();
            let mut expired: Vec<SId> = Vec::new();
            for (sid, deadline) in state.active_streams {
                if deadline > tick {
                    kept.insert(sid, deadline);
                } else {
                    expired.push(sid);
                }
            }

            let effects: Vec<Effect<WId, SId>> = expired
                .into_iter()
                .flat_map(|sid| {
                    [
                        Effect::SendClientError(SendClientError {
                            client_stream_id: sid.clone(),
                            message: String::from("stream timed out"),
                        }),
                        Effect::SendClientDone(SendClientDone {
                            client_stream_id: sid,
                        }),
                    ]
                })
                .collect();

            Transition {
                state: GatewayState {
                    tick,
                    available,
                    active_streams: kept,
                    ..state
                },
                effects,
            }
        }
        Event::SessionEvent {
            session_id,
            event: session_event,
        } => match state.sessions.extract(&session_id) {
            Some((session_state, sessions)) => {
                let session::Transition {
                    state: updated_session_state,
                    effects: session_effects,
                } = session::kernel::reduce(session_state, session_event);

                Transition {
                    state: GatewayState {
                        sessions: sessions.update(session_id, updated_session_state),
                        ..state
                    },
                    effects: session_effects
                        .into_iter()
                        .map(Effect::SessionEffect)
                        .collect(),
                }
            }
            None => Transition {
                state,
                effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                    worker_description: String::from("unknown"),
                    message: format!("event for unknown session {}", session_id),
                })],
            },
        },
    }
}

fn first_capable_worker_id<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash>(
    state: &GatewayState<WId, SId>,
    required: &Capability,
) -> Option<WId> {
    state
        .available
        .iter()
        .find(|(_, entry)| entry.capabilities.contains(required))
        .map(|(wid, _)| wid.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    type WId = String;
    type SId = String;

    fn w(value: &str) -> WId {
        String::from(value)
    }

    fn s(value: &str) -> SId {
        String::from(value)
    }

    fn chat_caps() -> HashSet<Capability> {
        HashSet::unit(Capability::Chat)
    }

    fn transcription_caps() -> HashSet<Capability> {
        HashSet::unit(Capability::Transcription)
    }

    fn both_caps() -> HashSet<Capability> {
        vec![Capability::Chat, Capability::Transcription]
            .into_iter()
            .collect()
    }

    fn chat_entry(deadline: u64) -> WorkerEntry {
        WorkerEntry {
            deadline,
            capabilities: chat_caps(),
        }
    }

    #[test]
    fn stream_false_emits_error_and_done() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let client_stream_id = s("client-1");
        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: client_stream_id.clone(),

                payload: json!({"model": "demo"}),
                stream: false,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: client_stream_id.clone(),
                    message: String::from("stream=true is required in this phase"),
                }),
                Effect::SendClientDone(SendClientDone { client_stream_id }),
            ]
        );
    }

    #[test]
    fn stream_true_assigns_first_available_worker() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));
        state.available.insert(w("worker-b"), chat_entry(100));
        let client_stream_id = s("client-2");
        let payload = json!({"model": "demo"});
        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: client_stream_id.clone(),

                payload: payload.clone(),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-a")
                && e.client_stream_id == client_stream_id
                && e.payload == payload
        ));
        // Worker consumed — gone from available
        assert!(!transition.state.available.contains_key(&w("worker-a")));
        // Other worker still available
        assert!(transition.state.available.contains_key(&w("worker-b")));
        // Stream is now active
        assert!(transition.state.active_streams.contains_key(&s("client-2")));
    }

    #[test]
    fn stream_true_without_available_worker_emits_error_and_done() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let client_stream_id = s("client-3");
        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: client_stream_id.clone(),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: client_stream_id.clone(),
                    message: String::from("no idle worker available"),
                }),
                Effect::SendClientDone(SendClientDone { client_stream_id }),
            ]
        );
    }

    #[test]
    fn dispatched_worker_is_consumed_from_available() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert!(transition.state.available.is_empty());
        assert!(transition.state.active_streams.contains_key(&s("client-1")));
    }

    #[test]
    fn dispatch_sets_stream_deadline() {
        let mut state = GatewayState::new(60, 10);
        state.tick = 5;
        state.available.insert(w("worker-a"), chat_entry(100));

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(
            transition.state.active_streams.get(&s("client-1")),
            Some(&15) // tick 5 + stream_ttl 10
        );
    }

    #[test]
    fn registration_sets_worker_deadline() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(20, 30);
        state.tick = 3;

        let transition = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-a"),
                capabilities: chat_caps(),
            },
        );

        assert_eq!(
            transition
                .state
                .available
                .get(&w("worker-a"))
                .map(|e| e.deadline),
            Some(23) // tick 3 + worker_ttl 20
        );
    }

    #[test]
    fn second_request_rejected_when_no_workers_available() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let first = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );
        assert!(matches!(first.effects.as_slice(), [Effect::DispatchJob(_)]));

        let second = reduce(
            first.state,
            Event::HttpChatRequested {
                client_stream_id: s("client-2"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );
        assert_eq!(
            second.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: s("client-2"),
                    message: String::from("no idle worker available"),
                }),
                Effect::SendClientDone(SendClientDone {
                    client_stream_id: s("client-2"),
                }),
            ]
        );
    }

    #[test]
    fn fresh_registration_after_assignment_cleared_allows_new_dispatch() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let dispatched = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        let cleared = reduce(
            dispatched.state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        );
        assert_eq!(
            cleared.effects,
            vec![Effect::SendClientDone(SendClientDone {
                client_stream_id: s("client-1"),
            })]
        );

        let re_registered = reduce(
            cleared.state,
            Event::WorkerRegistered {
                worker_id: w("worker-b"),
                capabilities: chat_caps(),
            },
        );

        let second = reduce(
            re_registered.state,
            Event::HttpChatRequested {
                client_stream_id: s("client-2"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );
        assert_eq!(second.effects.len(), 1);
        assert!(matches!(
            &second.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-b")
        ));
    }

    #[test]
    fn assignment_cleared_emits_done() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let dispatched = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        let cleared = reduce(
            dispatched.state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        );

        assert_eq!(
            cleared.effects,
            vec![Effect::SendClientDone(SendClientDone {
                client_stream_id: s("client-1"),
            })]
        );
        assert!(cleared.state.active_streams.is_empty());
    }

    #[test]
    fn assignment_cleared_for_unknown_stream_emits_protocol_violation() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let transition = reduce(
            state,
            Event::AssignmentCleared {
                client_stream_id: s("unknown-stream"),
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    #[test]
    fn duplicate_worker_registered_emits_protocol_violation() {
        let mut state: GatewayState<WId, SId> = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let transition = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-a"),
                capabilities: chat_caps(),
            },
        );

        assert_eq!(
            transition.effects,
            vec![Effect::ProtocolViolation(ProtocolViolation {
                worker_description: w("worker-a"),
                message: String::from("duplicate worker registration"),
            })]
        );
        assert!(transition.state.available.contains_key(&w("worker-a")));
    }

    #[test]
    fn worker_registration_adds_to_available() {
        let state: GatewayState<WId, SId> = GatewayState::default();

        let transition = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-a"),
                capabilities: chat_caps(),
            },
        );

        assert!(transition.effects.is_empty());
        assert!(transition.state.available.contains_key(&w("worker-a")));
    }

    #[test]
    fn tick_increments_counter() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        assert_eq!(state.tick, 0);

        let t1 = reduce(state, Event::Tick);
        assert_eq!(t1.state.tick, 1);

        let t2 = reduce(t1.state, Event::Tick);
        assert_eq!(t2.state.tick, 2);
    }

    #[test]
    fn tick_with_no_entries_emits_no_effects() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let transition = reduce(state, Event::Tick);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn worker_heartbeat_resets_deadline() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(5, 5);
        state.tick = 2;
        state.available.insert(w("worker-a"), chat_entry(5));

        let transition = reduce(
            state,
            Event::WorkerHeartbeat {
                worker_id: w("worker-a"),
            },
        );

        assert!(transition.effects.is_empty());
        assert_eq!(
            transition
                .state
                .available
                .get(&w("worker-a"))
                .map(|e| e.deadline),
            Some(7) // tick 2 + worker_ttl 5
        );
    }

    #[test]
    fn worker_heartbeat_unknown_emits_protocol_violation() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let transition = reduce(
            state,
            Event::WorkerHeartbeat {
                worker_id: w("ghost"),
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown worker")
        ));
    }

    #[test]
    fn stream_heartbeat_resets_deadline() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(5, 10);
        state.tick = 3;
        state.active_streams.insert(s("client-1"), 8);

        let transition = reduce(
            state,
            Event::StreamHeartbeat {
                client_stream_id: s("client-1"),
            },
        );

        assert!(transition.effects.is_empty());
        assert_eq!(
            transition.state.active_streams.get(&s("client-1")),
            Some(&13) // tick 3 + stream_ttl 10
        );
    }

    #[test]
    fn stream_heartbeat_unknown_emits_protocol_violation() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let transition = reduce(
            state,
            Event::StreamHeartbeat {
                client_stream_id: s("ghost"),
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    #[test]
    fn tick_does_not_expire_entries_before_deadline() {
        let mut state = GatewayState::new(5, 5);
        state.available.insert(w("worker-a"), chat_entry(5));
        state.active_streams.insert(s("client-1"), 5);

        // Tick 1 — deadline is 5, not reached yet
        let transition = reduce(state, Event::Tick);
        assert!(transition.effects.is_empty());
        assert!(transition.state.available.contains_key(&w("worker-a")));
        assert!(transition.state.active_streams.contains_key(&s("client-1")));
    }

    #[test]
    fn worker_expires_after_ttl_ticks() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(3, 30);
        state.available.insert(w("worker-a"), chat_entry(3));

        // Ticks 1, 2: not expired
        let state = reduce(state, Event::Tick).state;
        assert!(state.available.contains_key(&w("worker-a")));
        let state = reduce(state, Event::Tick).state;
        assert!(state.available.contains_key(&w("worker-a")));

        // Tick 3: deadline reached, worker expired
        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.is_empty());
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn stream_expires_after_ttl_ticks() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(60, 2);
        state.active_streams.insert(s("client-1"), 2);

        // Tick 1: not expired
        let state = reduce(state, Event::Tick).state;
        assert!(state.active_streams.contains_key(&s("client-1")));

        // Tick 2: deadline reached, stream expired
        let transition = reduce(state, Event::Tick);
        assert!(transition.state.active_streams.is_empty());
        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: s("client-1"),
                    message: String::from("stream timed out"),
                }),
                Effect::SendClientDone(SendClientDone {
                    client_stream_id: s("client-1"),
                }),
            ]
        );
    }

    #[test]
    fn heartbeat_prevents_expiration() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(2, 2);
        state.available.insert(w("worker-a"), chat_entry(2));
        state.active_streams.insert(s("client-1"), 2);

        // Tick 1
        let state = reduce(state, Event::Tick).state;

        // Heartbeat at tick 1 -> new deadlines = 1 + 2 = 3
        let state = reduce(
            state,
            Event::WorkerHeartbeat {
                worker_id: w("worker-a"),
            },
        )
        .state;
        let state = reduce(
            state,
            Event::StreamHeartbeat {
                client_stream_id: s("client-1"),
            },
        )
        .state;

        // Tick 2: would have expired without heartbeat, but deadline is now 3
        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.contains_key(&w("worker-a")));
        assert!(transition.state.active_streams.contains_key(&s("client-1")));
        assert!(transition.effects.is_empty());

        // Tick 3: now expires
        let transition = reduce(transition.state, Event::Tick);
        assert!(transition.state.available.is_empty());
        assert!(transition.state.active_streams.is_empty());
    }

    #[test]
    fn multiple_expirations_in_single_tick() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(1, 1);
        state.available.insert(w("worker-a"), chat_entry(1));
        state.available.insert(w("worker-b"), chat_entry(1));
        state.active_streams.insert(s("client-1"), 1);
        state.active_streams.insert(s("client-2"), 1);

        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.is_empty());
        assert!(transition.state.active_streams.is_empty());

        // Two streams expired -> 4 effects (error + done for each)
        assert_eq!(transition.effects.len(), 4);
        let error_streams: Vec<_> = transition
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::SendClientError(e) => Some(e.client_stream_id.clone()),
                _ => None,
            })
            .collect();
        assert!(error_streams.contains(&s("client-1")));
        assert!(error_streams.contains(&s("client-2")));
    }

    #[test]
    fn assignment_cleared_before_timeout_no_timeout_effects() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(60, 3);
        state.available.insert(w("worker-a"), chat_entry(100));

        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        // Clear before timeout
        let state = reduce(
            state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        )
        .state;

        // Tick past the original deadline — no effects, stream already gone
        let state = reduce(state, Event::Tick).state;
        let state = reduce(state, Event::Tick).state;
        let transition = reduce(state, Event::Tick);
        assert!(transition.effects.is_empty());
        assert!(transition.state.active_streams.is_empty());
    }

    #[test]
    fn assignment_cleared_after_timeout_emits_protocol_violation() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(60, 1);
        state.active_streams.insert(s("client-1"), 1);

        // Tick 1: stream expires
        let state = reduce(state, Event::Tick).state;
        assert!(state.active_streams.is_empty());

        // Late AssignmentCleared -> protocol violation (unknown stream)
        let transition = reduce(
            state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        );
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    // --- AssignmentFailed tests ---

    #[test]
    fn assignment_failed_emits_error_and_done() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        let transition = reduce(
            state,
            Event::AssignmentFailed {
                client_stream_id: s("client-1"),
                message: String::from("worker channel closed"),
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: s("client-1"),
                    message: String::from("worker channel closed"),
                }),
                Effect::SendClientDone(SendClientDone {
                    client_stream_id: s("client-1"),
                }),
            ]
        );
        assert!(transition.state.active_streams.is_empty());
    }

    #[test]
    fn assignment_failed_unknown_stream_emits_protocol_violation() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let transition = reduce(
            state,
            Event::AssignmentFailed {
                client_stream_id: s("unknown-stream"),
                message: String::from("some error"),
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    #[test]
    fn double_assignment_failed_second_emits_protocol_violation() {
        let mut state: GatewayState<WId, SId> = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        // First failure succeeds
        let first = reduce(
            state,
            Event::AssignmentFailed {
                client_stream_id: s("client-1"),
                message: String::from("worker crashed"),
            },
        );
        assert_eq!(first.effects.len(), 2); // error + done

        // Second failure is protocol violation
        let second = reduce(
            first.state,
            Event::AssignmentFailed {
                client_stream_id: s("client-1"),
                message: String::from("late failure"),
            },
        );
        assert_eq!(second.effects.len(), 1);
        assert!(matches!(
            &second.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    #[test]
    fn assignment_failed_before_timeout_no_timeout_effects() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(60, 3);
        state.available.insert(w("worker-a"), chat_entry(100));

        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        // Fail before timeout
        let state = reduce(
            state,
            Event::AssignmentFailed {
                client_stream_id: s("client-1"),
                message: String::from("dispatch failed"),
            },
        )
        .state;

        // Tick past the original deadline — no effects, stream already gone
        let state = reduce(state, Event::Tick).state;
        let state = reduce(state, Event::Tick).state;
        let transition = reduce(state, Event::Tick);
        assert!(transition.effects.is_empty());
        assert!(transition.state.active_streams.is_empty());
    }

    // --- State mutation boundary tests ---

    #[test]
    fn stream_false_does_not_mutate_state() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));
        state.tick = 5;
        let state_before = state.clone();

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: false,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(transition.state, state_before);
    }

    #[test]
    fn stream_true_no_workers_does_not_mutate_state() {
        let state: GatewayState<WId, SId> = GatewayState::default();
        let state_before = state.clone();

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(transition.state, state_before);
    }

    #[test]
    fn multiple_distinct_registrations_all_succeed() {
        let state: GatewayState<WId, SId> = GatewayState::default();

        let state = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-a"),
                capabilities: chat_caps(),
            },
        )
        .state;
        let state = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-b"),
                capabilities: chat_caps(),
            },
        )
        .state;
        let state = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-c"),
                capabilities: chat_caps(),
            },
        )
        .state;

        assert_eq!(state.available.len(), 3);
        assert!(state.available.contains_key(&w("worker-a")));
        assert!(state.available.contains_key(&w("worker-b")));
        assert!(state.available.contains_key(&w("worker-c")));
    }

    #[test]
    fn double_assignment_cleared_second_emits_protocol_violation() {
        let mut state: GatewayState<WId, SId> = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        // First clear succeeds
        let first = reduce(
            state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        );
        assert!(matches!(
            first.effects.as_slice(),
            [Effect::SendClientDone(_)]
        ));

        // Second clear is protocol violation
        let second = reduce(
            first.state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        );
        assert_eq!(second.effects.len(), 1);
        assert!(matches!(
            &second.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    #[test]
    fn duplicate_stream_id_rejected_with_error_and_done() {
        let mut state: GatewayState<WId, SId> = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));
        state.available.insert(w("worker-b"), chat_entry(100));

        // First dispatch succeeds
        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;
        assert!(state.active_streams.contains_key(&s("client-1")));

        // Second dispatch with same stream ID is rejected
        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: s("client-1"),
                    message: String::from("stream already has an active assignment"),
                }),
                Effect::SendClientDone(SendClientDone {
                    client_stream_id: s("client-1"),
                }),
            ]
        );
        // State unchanged — worker-b still available, stream-1 still active
        assert!(transition.state.available.contains_key(&w("worker-b")));
        assert!(transition.state.active_streams.contains_key(&s("client-1")));
    }

    #[test]
    fn heartbeat_for_dispatched_worker_emits_protocol_violation() {
        let mut state: GatewayState<WId, SId> = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        // Dispatch consumes the worker
        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        // Stale heartbeat for consumed worker
        let transition = reduce(
            state,
            Event::WorkerHeartbeat {
                worker_id: w("worker-a"),
            },
        );
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown worker")
        ));
    }

    #[test]
    fn heartbeat_for_cleared_stream_emits_protocol_violation() {
        let mut state: GatewayState<WId, SId> = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let state = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        )
        .state;

        let state = reduce(
            state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        )
        .state;

        // Stale heartbeat for cleared stream
        let transition = reduce(
            state,
            Event::StreamHeartbeat {
                client_stream_id: s("client-1"),
            },
        );
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::ProtocolViolation(pv) if pv.message.contains("unknown stream")
        ));
    }

    #[test]
    fn mixed_deadlines_only_expired_entries_removed() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(60, 60);
        state.available.insert(w("worker-stale"), chat_entry(2));
        state.available.insert(w("worker-fresh"), chat_entry(10));
        state.active_streams.insert(s("stream-stale"), 2);
        state.active_streams.insert(s("stream-fresh"), 10);

        // Tick to 2: stale entries expire, fresh ones survive
        let state = reduce(state, Event::Tick).state;
        let transition = reduce(state, Event::Tick);

        assert!(!transition.state.available.contains_key(&w("worker-stale")));
        assert!(transition.state.available.contains_key(&w("worker-fresh")));
        assert!(
            !transition
                .state
                .active_streams
                .contains_key(&s("stream-stale"))
        );
        assert!(
            transition
                .state
                .active_streams
                .contains_key(&s("stream-fresh"))
        );

        // Only the stale stream produced effects
        assert_eq!(transition.effects.len(), 2);
        assert!(matches!(
            &transition.effects[0],
            Effect::SendClientError(e) if e.client_stream_id == s("stream-stale")
        ));
    }

    #[test]
    fn worker_expiration_does_not_affect_active_streams() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(1, 60);
        state.available.insert(w("worker-a"), chat_entry(1));
        state.active_streams.insert(s("client-1"), 100);

        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.is_empty());
        assert!(transition.state.active_streams.contains_key(&s("client-1")));
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn stream_expiration_does_not_affect_available_workers() {
        let mut state: GatewayState<WId, SId> = GatewayState::new(60, 1);
        state.available.insert(w("worker-a"), chat_entry(100));
        state.active_streams.insert(s("client-1"), 1);

        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.contains_key(&w("worker-a")));
        assert!(transition.state.active_streams.is_empty());
        assert_eq!(transition.effects.len(), 2); // error + done
    }

    #[test]
    fn zero_ttl_expires_on_next_tick() {
        let state: GatewayState<WId, SId> = GatewayState::new(0, 0);

        // Register at tick 0 -> deadline = 0 + 0 = 0
        let state = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-a"),
                capabilities: chat_caps(),
            },
        )
        .state;
        assert_eq!(
            state.available.get(&w("worker-a")).map(|e| e.deadline),
            Some(0)
        );

        // Tick 1: deadline 0 <= tick 1, expired
        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.is_empty());
    }

    #[test]
    fn tick_preserves_ttl_config() {
        let state: GatewayState<WId, SId> = GatewayState::new(42, 17);
        let transition = reduce(state, Event::Tick);
        assert_eq!(transition.state.worker_ttl, 42);
        assert_eq!(transition.state.stream_ttl, 17);
    }

    // --- Capability routing tests ---

    #[test]
    fn chat_job_dispatches_to_chat_capable_worker() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-a")
        ));
    }

    #[test]
    fn chat_job_skips_transcription_only_worker() {
        let mut state = GatewayState::default();
        state.available.insert(
            w("worker-a"),
            WorkerEntry {
                deadline: 100,
                capabilities: transcription_caps(),
            },
        );

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: s("client-1"),
                    message: String::from("no idle worker available"),
                }),
                Effect::SendClientDone(SendClientDone {
                    client_stream_id: s("client-1"),
                }),
            ]
        );
        // Worker not consumed
        assert!(transition.state.available.contains_key(&w("worker-a")));
    }

    #[test]
    fn transcription_job_dispatches_to_transcription_capable_worker() {
        let mut state = GatewayState::default();
        state.available.insert(
            w("worker-a"),
            WorkerEntry {
                deadline: 100,
                capabilities: transcription_caps(),
            },
        );

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({}),
                stream: true,
                required_capability: Capability::Transcription,
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-a")
        ));
    }

    #[test]
    fn multi_capable_worker_serves_either_job_type() {
        let mut state = GatewayState::default();
        state.available.insert(
            w("worker-a"),
            WorkerEntry {
                deadline: 100,
                capabilities: both_caps(),
            },
        );

        // Chat request dispatches
        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({"model": "demo"}),
                stream: true,
                required_capability: Capability::Chat,
            },
        );
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-a")
        ));

        // Re-register and try transcription
        let state = reduce(
            transition.state,
            Event::AssignmentCleared {
                client_stream_id: s("client-1"),
            },
        )
        .state;
        let state = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-b"),
                capabilities: both_caps(),
            },
        )
        .state;

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-2"),

                payload: json!({}),
                stream: true,
                required_capability: Capability::Transcription,
            },
        );
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-b")
        ));
    }

    #[test]
    fn selects_capable_worker_when_mixed_pool() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));
        state.available.insert(
            w("worker-b"),
            WorkerEntry {
                deadline: 100,
                capabilities: transcription_caps(),
            },
        );

        // Transcription request should skip worker-a (chat only) and pick worker-b
        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({}),
                stream: true,
                required_capability: Capability::Transcription,
            },
        );

        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::DispatchJob(e) if e.worker_id == w("worker-b")
        ));
        // worker-a untouched
        assert!(transition.state.available.contains_key(&w("worker-a")));
    }

    #[test]
    fn no_capable_worker_available_returns_error() {
        let mut state = GatewayState::default();
        state.available.insert(w("worker-a"), chat_entry(100));
        state.available.insert(w("worker-b"), chat_entry(100));

        let transition = reduce(
            state,
            Event::HttpChatRequested {
                client_stream_id: s("client-1"),

                payload: json!({}),
                stream: true,
                required_capability: Capability::Transcription,
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                Effect::SendClientError(SendClientError {
                    client_stream_id: s("client-1"),
                    message: String::from("no idle worker available"),
                }),
                Effect::SendClientDone(SendClientDone {
                    client_stream_id: s("client-1"),
                }),
            ]
        );
        // Neither worker consumed
        assert_eq!(transition.state.available.len(), 2);
    }

    #[test]
    fn registration_stores_capabilities() {
        let state: GatewayState<WId, SId> = GatewayState::default();

        let transition = reduce(
            state,
            Event::WorkerRegistered {
                worker_id: w("worker-a"),
                capabilities: both_caps(),
            },
        );

        let entry = transition.state.available.get(&w("worker-a")).unwrap();
        assert_eq!(entry.capabilities, both_caps());
    }

    // --- Property tests ---

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        // Small ID pools to encourage collisions and interesting interactions
        const WORKER_IDS: &[&str] = &["w0", "w1", "w2"];
        const STREAM_IDS: &[&str] = &["s0", "s1", "s2"];
        const SESSION_IDS: &[&str] = &["sess0", "sess1"];

        fn arb_capability() -> impl Strategy<Value = Capability> {
            prop_oneof![Just(Capability::Chat), Just(Capability::Transcription),]
        }

        fn arb_capabilities() -> impl Strategy<Value = HashSet<Capability>> {
            proptest::collection::btree_set(arb_capability(), 1..=2)
                .prop_map(|s| s.into_iter().collect())
        }

        fn arb_event() -> impl Strategy<Value = Event<WId, SId>> {
            prop_oneof![
                // HttpChatRequested with random capability
                (
                    prop::sample::select(STREAM_IDS),
                    any::<bool>(),
                    arb_capability(),
                )
                    .prop_map(|(sid, stream, cap)| {
                        Event::HttpChatRequested {
                            client_stream_id: s(sid),

                            payload: json!({"model": "demo"}),
                            stream,
                            required_capability: cap,
                        }
                    }),
                // WorkerRegistered with random capabilities
                (prop::sample::select(WORKER_IDS), arb_capabilities()).prop_map(|(wid, caps)| {
                    Event::WorkerRegistered {
                        worker_id: w(wid),
                        capabilities: caps,
                    }
                }),
                // AssignmentCleared
                prop::sample::select(STREAM_IDS).prop_map(|sid| Event::AssignmentCleared {
                    client_stream_id: s(sid),
                }),
                // AssignmentFailed
                prop::sample::select(STREAM_IDS).prop_map(|sid| Event::AssignmentFailed {
                    client_stream_id: s(sid),
                    message: String::from("test failure"),
                }),
                // WorkerHeartbeat
                prop::sample::select(WORKER_IDS)
                    .prop_map(|wid| Event::WorkerHeartbeat { worker_id: w(wid) }),
                // StreamHeartbeat
                prop::sample::select(STREAM_IDS).prop_map(|sid| Event::StreamHeartbeat {
                    client_stream_id: s(sid),
                }),
                // Tick
                Just(Event::Tick),
                // SessionEvent
                (
                    prop::sample::select(SESSION_IDS),
                    prop_oneof![
                        Just(crate::gateway::session::Event::EntryAppended {
                            payload: json!({"data": "test"}),
                        }),
                        prop::sample::select(STREAM_IDS).prop_map(|sub| {
                            crate::gateway::session::Event::Subscribed {
                                subscriber_id: s(sub),
                            }
                        }),
                        prop::sample::select(STREAM_IDS).prop_map(|sub| {
                            crate::gateway::session::Event::Unsubscribed {
                                subscriber_id: s(sub),
                            }
                        }),
                    ],
                )
                    .prop_map(|(sess, event)| Event::SessionEvent {
                        session_id: SessionId(String::from(sess)),
                        event,
                    }),
            ]
        }

        fn arb_event_sequence() -> impl Strategy<Value = Vec<Event<WId, SId>>> {
            proptest::collection::vec(arb_event(), 0..100)
        }

        // I1: Workers only exist in `available`. Once dispatched, they leave
        // kernel state entirely. (We can verify: available has no duplicate
        // with active_streams since they're different types — this is structural.
        // What we check: available keys are unique — guaranteed by BTreeMap.)
        //
        // I2: Every stream removed from `active_streams` produces SendClientDone.
        // We check this per-transition.
        //
        // I3: Every DispatchJob is emitted in the same transition that removes
        // a worker from available and adds a stream to active_streams.
        //
        // I4: No silent state changes — if state changed, effects were emitted.
        // Exception: WorkerRegistered adding a worker, Tick incrementing counter,
        // Tick expiring workers (no client).
        //
        // I5: Every stream that enters active_streams eventually gets terminal
        // effects (via AssignmentCleared or Tick expiration). We check this at
        // the end by running enough ticks to expire everything.
        //
        // I6: Every DispatchJob dispatches to a worker that had the required
        // capability at dispatch time.
        proptest! {
            #[test]
            fn invariant_i2_stream_removal_produces_done(
                events in arb_event_sequence()
            ) {
                // I2: every stream removed from active_streams in a transition
                // must have a SendClientDone in the effects.
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                for event in events {
                    let streams_before: std::collections::BTreeSet<_> =
                        state.active_streams.keys().cloned().collect();

                    let transition = reduce(state, event);
                    let streams_after: std::collections::BTreeSet<_> =
                        transition.state.active_streams.keys().cloned().collect();

                    // Streams that were removed
                    let removed: Vec<_> = streams_before
                        .difference(&streams_after)
                        .cloned()
                        .collect();

                    for sid in &removed {
                        let has_done = transition.effects.iter().any(|e| matches!(
                            e,
                            Effect::SendClientDone(d) if d.client_stream_id == *sid
                        ));
                        prop_assert!(
                            has_done,
                            "stream {} removed from active_streams without SendClientDone",
                            sid
                        );
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn invariant_i3_dispatch_atomicity(
                events in arb_event_sequence()
            ) {
                // I3: every DispatchJob effect is emitted in a transition that
                // removes a worker from available AND adds a stream to active_streams.
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                for event in events {
                    let workers_before: std::collections::BTreeSet<_> =
                        state.available.keys().cloned().collect();
                    let streams_before: std::collections::BTreeSet<_> =
                        state.active_streams.keys().cloned().collect();

                    let transition = reduce(state, event);

                    let dispatch_effects: Vec<_> = transition.effects.iter().filter(|e| {
                        matches!(e, Effect::DispatchJob(_))
                    }).collect();

                    for effect in dispatch_effects {
                        if let Effect::DispatchJob(job) = effect {
                            // Worker was in available before, not after
                            prop_assert!(
                                workers_before.contains(&job.worker_id),
                                "DispatchJob references worker {} not in previous available",
                                job.worker_id
                            );
                            prop_assert!(
                                !transition.state.available.contains_key(&job.worker_id),
                                "DispatchJob worker {} still in available after dispatch",
                                job.worker_id
                            );

                            // Stream was not in active_streams before, is after
                            prop_assert!(
                                !streams_before.contains(&job.client_stream_id),
                                "DispatchJob stream {} was already in active_streams",
                                job.client_stream_id
                            );
                            prop_assert!(
                                transition.state.active_streams.contains_key(&job.client_stream_id),
                                "DispatchJob stream {} not added to active_streams",
                                job.client_stream_id
                            );
                        }
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn invariant_i4_no_silent_state_changes(
                events in arb_event_sequence()
            ) {
                // I4: if state changed in a way that affects clients, effects
                // were emitted. Allowed silent changes: worker added to available,
                // tick counter incremented, stale worker expired.
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                for event in events {
                    let streams_before: std::collections::BTreeSet<_> =
                        state.active_streams.keys().cloned().collect();

                    let transition = reduce(state, event);
                    let streams_after: std::collections::BTreeSet<_> =
                        transition.state.active_streams.keys().cloned().collect();

                    // If any stream was removed, there must be effects
                    let removed_streams: Vec<_> = streams_before
                        .difference(&streams_after)
                        .collect();

                    if !removed_streams.is_empty() {
                        prop_assert!(
                            !transition.effects.is_empty(),
                            "streams removed without any effects: {:?}",
                            removed_streams
                        );
                    }

                    // If any stream was added, there must be a DispatchJob effect
                    let added_streams: Vec<_> = streams_after
                        .difference(&streams_before)
                        .collect();

                    for sid in &added_streams {
                        let has_dispatch = transition.effects.iter().any(|e| matches!(
                            e,
                            Effect::DispatchJob(j) if j.client_stream_id == **sid
                        ));
                        prop_assert!(
                            has_dispatch,
                            "stream {} added to active_streams without DispatchJob",
                            sid
                        );
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn invariant_i5_all_streams_eventually_terminate(
                events in arb_event_sequence()
            ) {
                // I5: every stream that enters the kernel eventually gets terminal
                // effects. We apply the event sequence, then pump enough ticks to
                // expire everything, and verify active_streams is empty.
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                // Track all streams that ever entered active_streams
                let mut all_streams_seen: std::collections::BTreeSet<SId> =
                    std::collections::BTreeSet::new();
                // Track streams that got SendClientDone
                let mut streams_done: std::collections::BTreeSet<SId> =
                    std::collections::BTreeSet::new();

                for event in events {
                    let transition = reduce(state, event);

                    for sid in transition.state.active_streams.keys() {
                        all_streams_seen.insert(sid.clone());
                    }

                    for effect in &transition.effects {
                        if let Effect::SendClientDone(d) = effect {
                            streams_done.insert(d.client_stream_id.clone());
                        }
                    }

                    state = transition.state;
                }

                // Pump enough ticks to expire any remaining active streams
                // (stream_ttl is 3, so 4 ticks is sufficient)
                for _ in 0..10 {
                    let transition = reduce(state, Event::Tick);

                    for effect in &transition.effects {
                        if let Effect::SendClientDone(d) = effect {
                            streams_done.insert(d.client_stream_id.clone());
                        }
                    }

                    state = transition.state;
                }

                // All streams that were ever active must have received SendClientDone
                for sid in &all_streams_seen {
                    prop_assert!(
                        streams_done.contains(sid),
                        "stream {} entered active_streams but never got SendClientDone",
                        sid
                    );
                }

                // No lingering active streams
                prop_assert!(
                    state.active_streams.is_empty(),
                    "active_streams not empty after drain: {:?}",
                    state.active_streams.keys().collect::<Vec<_>>()
                );
            }

            #[test]
            fn tick_counter_is_monotonic(
                events in arb_event_sequence()
            ) {
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);
                let mut prev_tick = state.tick;

                for event in events {
                    let transition = reduce(state, event);
                    prop_assert!(
                        transition.state.tick >= prev_tick,
                        "tick went backwards: {} -> {}",
                        prev_tick,
                        transition.state.tick
                    );
                    prev_tick = transition.state.tick;
                    state = transition.state;
                }
            }

            #[test]
            fn stream_timeout_always_emits_error_and_done(
                events in arb_event_sequence()
            ) {
                // When a stream is expired by Tick, it must emit both
                // SendClientError and SendClientDone for that stream.
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                for event in events {
                    let streams_before: std::collections::BTreeSet<_> =
                        state.active_streams.keys().cloned().collect();
                    let is_tick = matches!(event, Event::Tick);

                    let transition = reduce(state, event);

                    if is_tick {
                        let streams_after: std::collections::BTreeSet<_> =
                            transition.state.active_streams.keys().cloned().collect();

                        let expired: Vec<_> = streams_before
                            .difference(&streams_after)
                            .cloned()
                            .collect();

                        for sid in &expired {
                            let has_error = transition.effects.iter().any(|e| matches!(
                                e,
                                Effect::SendClientError(err) if err.client_stream_id == *sid
                            ));
                            let has_done = transition.effects.iter().any(|e| matches!(
                                e,
                                Effect::SendClientDone(d) if d.client_stream_id == *sid
                            ));
                            prop_assert!(
                                has_error && has_done,
                                "stream {} expired by tick without error+done pair",
                                sid
                            );
                        }
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn invariant_i6_dispatch_respects_capability(
                events in arb_event_sequence()
            ) {
                // I6: every DispatchJob dispatches to a worker that had the
                // required capability at dispatch time.
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                for event in events {
                    // Snapshot worker capabilities before the transition
                    let worker_caps_before: std::collections::BTreeMap<WId, HashSet<Capability>> =
                        state
                            .available
                            .iter()
                            .map(|(wid, entry)| (wid.clone(), entry.capabilities.clone()))
                            .collect();

                    // Extract required capability if this is a job request
                    let required_cap = match &event {
                        Event::HttpChatRequested {
                            required_capability, ..
                        } => Some(*required_capability),
                        _ => None,
                    };

                    let transition = reduce(state, event);

                    for effect in &transition.effects {
                        if let Effect::DispatchJob(job) = effect
                            && let Some(cap) = required_cap
                        {
                            let worker_had_cap = worker_caps_before
                                .get(&job.worker_id)
                                .map(|caps| caps.contains(&cap))
                                .unwrap_or(false);
                            prop_assert!(
                                worker_had_cap,
                                "DispatchJob sent to worker {} which lacked capability {:?}",
                                job.worker_id,
                                cap
                            );
                        }
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn every_http_chat_requested_produces_terminal_effects(
                events in arb_event_sequence()
            ) {
                // Every HttpChatRequested produces terminal effects in the
                // same transition: either a DispatchJob (post-dispatch path,
                // where I2/I5 cover eventual termination) or
                // SendClientError + SendClientDone (pre-dispatch rejection).
                let mut state: GatewayState<WId, SId> = GatewayState::new(3, 3);

                for event in events {
                    let is_http_chat = matches!(event, Event::HttpChatRequested { .. });

                    let streams_before: std::collections::BTreeSet<_> =
                        state.active_streams.keys().cloned().collect();

                    let transition = reduce(state, event);

                    if is_http_chat {
                        let streams_after: std::collections::BTreeSet<_> =
                            transition.state.active_streams.keys().cloned().collect();

                        let dispatched = streams_after
                            .difference(&streams_before)
                            .next()
                            .is_some();

                        if dispatched {
                            // Post-dispatch: must have DispatchJob
                            let has_dispatch = transition.effects.iter().any(|e|
                                matches!(e, Effect::DispatchJob(_))
                            );
                            prop_assert!(
                                has_dispatch,
                                "HttpChatRequested dispatched but no DispatchJob effect"
                            );
                        } else {
                            // Pre-dispatch rejection: must have SendClientError + SendClientDone
                            let has_error = transition.effects.iter().any(|e|
                                matches!(e, Effect::SendClientError(_))
                            );
                            let has_done = transition.effects.iter().any(|e|
                                matches!(e, Effect::SendClientDone(_))
                            );
                            prop_assert!(
                                has_error && has_done,
                                "HttpChatRequested rejected without SendClientError + SendClientDone"
                            );
                        }
                    }

                    state = transition.state;
                }
            }
        }
    }
}
