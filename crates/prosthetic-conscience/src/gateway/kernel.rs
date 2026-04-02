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
pub struct GatewayState<
    WId: Clone + Ord,
    SId: Clone + Eq + std::hash::Hash,
    SubId: Clone + Eq + std::hash::Hash,
> {
    pub tick: u64,
    pub worker_ttl: u64,
    pub stream_ttl: u64,
    pub subscriber_ttl: u64,
    pub runtime_id: u128,
    pub session_counter: u64,
    pub available: OrdMap<WId, WorkerEntry>,
    pub active_streams: HashMap<SId, u64>,
    pub sessions: HashMap<SessionId, session::State<SubId>>,
}

impl<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash, SubId: Clone + Eq + std::hash::Hash>
    GatewayState<WId, SId, SubId>
{
    pub fn new(runtime_id: u128, worker_ttl: u64, stream_ttl: u64, subscriber_ttl: u64) -> Self {
        Self {
            tick: 0,
            worker_ttl,
            stream_ttl,
            subscriber_ttl,
            runtime_id,
            session_counter: 0,
            available: OrdMap::new(),
            active_streams: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

impl<WId: Clone + Ord, SId: Clone + Eq + std::hash::Hash, SubId: Clone + Eq + std::hash::Hash>
    Default for GatewayState<WId, SId, SubId>
{
    fn default() -> Self {
        Self::new(0, 60, 30, 30)
    }
}

fn generate_session_id(runtime_id: u128, counter: u64) -> SessionId {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    runtime_id.hash(&mut hasher);
    counter.hash(&mut hasher);
    SessionId(format!("{:016x}", hasher.finish()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event<WId, SId, SubId> {
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
    SessionRequested {
        subscriber_id: SubId,
    },
    SessionEvent {
        session_id: SessionId,
        event: session::Event<SubId>,
    },
}

#[cfg(test)]
impl<WId, SId, SubId> Event<WId, SId, SubId> {
    /// Returns all SubIds referenced by this event.
    ///
    /// SAFETY-CRITICAL FOR INVARIANT TESTING: This method must use explicit
    /// match arms with NO wildcard (`_ =>`) pattern. When a new variant is
    /// added to this enum, the compiler will force the author to handle it
    /// here, ensuring they decide whether it carries a SubId. Returning an
    /// incorrect or incomplete result silently breaks property test P14
    /// (every SubId that enters the kernel eventually receives a terminal
    /// SubscriberRemoved effect), allowing subscriber handle leaks to go
    /// undetected.
    pub fn sub_ids(&self) -> Vec<&SubId> {
        match self {
            Event::HttpChatRequested { .. } => vec![],
            Event::WorkerRegistered { .. } => vec![],
            Event::AssignmentCleared { .. } => vec![],
            Event::AssignmentFailed { .. } => vec![],
            Event::WorkerHeartbeat { .. } => vec![],
            Event::StreamHeartbeat { .. } => vec![],
            Event::Tick => vec![],
            Event::SessionRequested { subscriber_id } => vec![subscriber_id],
            Event::SessionEvent { event, .. } => event.sub_ids(),
        }
    }
}

use super::effects::{
    dispatch_job::DispatchJob,
    protocol_violation::{ProtocolViolation, ViolationSource},
    send_client_done::SendClientDone,
    send_client_error::SendClientError,
};

#[derive(Debug, PartialEq)]
pub enum Effect<WId, SId, SubId> {
    DispatchJob(DispatchJob<WId, SId>),
    SendClientError(SendClientError<SId>),
    SendClientDone(SendClientDone<SId>),
    SessionEffect(session::Effect<SubId>),
    SessionCreated {
        session_id: SessionId,
        subscriber_id: SubId,
    },
    SessionExpired {
        session_id: SessionId,
        entries: Vec<Value>,
    },
    ProtocolViolation(ProtocolViolation),
}

pub struct Transition<
    WId: Clone + Ord,
    SId: Clone + Eq + std::hash::Hash,
    SubId: Clone + Eq + std::hash::Hash,
> {
    pub state: GatewayState<WId, SId, SubId>,
    pub effects: Vec<Effect<WId, SId, SubId>>,
}

pub fn reduce<WId, SId, SubId>(
    state: GatewayState<WId, SId, SubId>,
    event: Event<WId, SId, SubId>,
) -> Transition<WId, SId, SubId>
where
    WId: Clone + Ord + std::fmt::Display,
    SId: Clone + Eq + std::hash::Hash + std::fmt::Display,
    SubId: Clone + Eq + std::hash::Hash + std::fmt::Display,
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
                        source: ViolationSource::Worker(worker_id.to_string()),
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
                        source: ViolationSource::Stream(client_stream_id.to_string()),
                        message: String::from("assignment cleared for unknown stream"),
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
                        source: ViolationSource::Stream(client_stream_id.to_string()),
                        message: String::from("assignment failed for unknown stream"),
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
                    source: ViolationSource::Worker(worker_id.to_string()),
                    message: String::from("heartbeat from unknown worker"),
                })],
            },
        },
        Event::StreamHeartbeat { client_stream_id } => {
            if !state.active_streams.contains_key(&client_stream_id) {
                return Transition {
                    state,
                    effects: vec![Effect::ProtocolViolation(ProtocolViolation {
                        source: ViolationSource::Stream(client_stream_id.to_string()),
                        message: String::from("heartbeat for unknown stream"),
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

            let expired_streams: Vec<SId> = state
                .active_streams
                .iter()
                .filter(|(_, deadline)| **deadline <= tick)
                .map(|(sid, _)| sid.clone())
                .collect();
            let active_streams = expired_streams
                .iter()
                .fold(state.active_streams, |streams, sid| streams.without(sid));

            let effects: Vec<Effect<WId, SId, SubId>> = expired_streams
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

            // Propagate tick to all sessions
            let mut sessions = state.sessions;
            let mut session_effects: Vec<Effect<WId, SId, SubId>> = Vec::new();
            for (session_id, session_state) in sessions.clone() {
                let session::Transition {
                    state: updated,
                    effects: sess_effects,
                } = session::reduce(session_state, session::Event::Tick { tick });
                sessions = sessions.update(session_id.clone(), updated);
                session_effects.extend(sess_effects.into_iter().map(|e| Effect::SessionEffect(e)));
            }

            // Remove sessions with no subscribers and emit SessionExpired
            let expired_session_ids: Vec<SessionId> = sessions
                .iter()
                .filter(|(_, s)| s.subscribers.is_empty())
                .map(|(sid, _)| sid.clone())
                .collect();
            let (sessions, expiry_effects) = expired_session_ids.into_iter().fold(
                (sessions, Vec::new()),
                |(sessions, mut effects), session_id| {
                    // Safety: session_id came from iterating `sessions`, so extract always succeeds.
                    // Using match instead of expect to satisfy panic-free policy.
                    let Some((session_state, sessions)) = sessions.extract(&session_id) else {
                        return (sessions, effects);
                    };
                    effects.push(Effect::SessionExpired {
                        session_id,
                        entries: session_state.entries.into_entries(),
                    });
                    (sessions, effects)
                },
            );

            let mut all_effects = effects;
            all_effects.extend(session_effects);
            all_effects.extend(expiry_effects);

            Transition {
                state: GatewayState {
                    tick,
                    available,
                    active_streams,
                    sessions,
                    ..state
                },
                effects: all_effects,
            }
        }
        Event::SessionRequested { subscriber_id } => {
            let session_id = generate_session_id(state.runtime_id, state.session_counter);
            let session_counter = state.session_counter + 1;

            let initial_session = session::State {
                subscriber_ttl: state.subscriber_ttl,
                ..session::State::default()
            };
            let session::Transition {
                state: initial_session,
                ..
            } = session::reduce(
                initial_session,
                session::Event::Subscribed {
                    subscriber_id: subscriber_id.clone(),
                    tick: state.tick,
                },
            );

            Transition {
                state: GatewayState {
                    session_counter,
                    sessions: state.sessions.update(session_id.clone(), initial_session),
                    ..state
                },
                effects: vec![Effect::SessionCreated {
                    session_id,
                    subscriber_id,
                }],
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
            None => {
                let mut effects = vec![Effect::ProtocolViolation(ProtocolViolation {
                    source: ViolationSource::Session(session_id.to_string()),
                    message: String::from("event for unknown session"),
                })];
                // Emit SubscriberRemoved for any SubId referenced in an
                // event targeting an unknown session. The kernel cannot
                // trust that the impure runtime layer already cleaned up
                // the subscriber handle — defensive cleanup here prevents
                // registry handle leaks regardless of runtime behavior.
                //
                // No wildcard arm: adding a new session::Event variant
                // is a compile error until handled here.
                let cleanup_sub_id = match session_event {
                    session::Event::EntryAppended { .. } => None,
                    session::Event::Subscribed { subscriber_id, .. } => Some(subscriber_id),
                    session::Event::Unsubscribed { subscriber_id } => Some(subscriber_id),
                    session::Event::Tick { .. } => None,
                    session::Event::SubscriberHeartbeat { subscriber_id, .. } => {
                        Some(subscriber_id)
                    }
                };
                if let Some(subscriber_id) = cleanup_sub_id {
                    effects.push(Effect::SessionEffect(session::Effect::SubscriberRemoved {
                        subscriber_id,
                    }));
                }
                Transition { state, effects }
            }
        },
    }
}

fn first_capable_worker_id<
    WId: Clone + Ord,
    SId: Clone + Eq + std::hash::Hash,
    SubId: Clone + Eq + std::hash::Hash,
>(
    state: &GatewayState<WId, SId, SubId>,
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
    type SubId = String;

    fn w(value: &str) -> WId {
        String::from(value)
    }

    fn s(value: &str) -> SId {
        String::from(value)
    }

    fn sub(value: &str) -> SubId {
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 10, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 20, 30, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
                source: ViolationSource::Worker(w("worker-a")),
                message: String::from("duplicate worker registration"),
            })]
        );
        assert!(transition.state.available.contains_key(&w("worker-a")));
    }

    #[test]
    fn worker_registration_adds_to_available() {
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();

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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
        assert_eq!(state.tick, 0);

        let t1 = reduce(state, Event::Tick);
        assert_eq!(t1.state.tick, 1);

        let t2 = reduce(t1.state, Event::Tick);
        assert_eq!(t2.state.tick, 2);
    }

    #[test]
    fn tick_with_no_entries_emits_no_effects() {
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
        let transition = reduce(state, Event::Tick);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn worker_heartbeat_resets_deadline() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 5, 5, 30);
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 5, 10, 30);
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 5, 5, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 30, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 2, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 2, 2, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 1, 1, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 3, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 1, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 3, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();

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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 60, 30);
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 1, 60, 30);
        state.available.insert(w("worker-a"), chat_entry(1));
        state.active_streams.insert(s("client-1"), 100);

        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.is_empty());
        assert!(transition.state.active_streams.contains_key(&s("client-1")));
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn stream_expiration_does_not_affect_available_workers() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 1, 30);
        state.available.insert(w("worker-a"), chat_entry(100));
        state.active_streams.insert(s("client-1"), 1);

        let transition = reduce(state, Event::Tick);
        assert!(transition.state.available.contains_key(&w("worker-a")));
        assert!(transition.state.active_streams.is_empty());
        assert_eq!(transition.effects.len(), 2); // error + done
    }

    #[test]
    fn zero_ttl_expires_on_next_tick() {
        let state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 0, 0, 30);

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
        let state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 42, 17, 30);
        let transition = reduce(state, Event::Tick);
        assert_eq!(transition.state.worker_ttl, 42);
        assert_eq!(transition.state.stream_ttl, 17);
    }

    // --- Capability routing tests ---

    #[test]
    fn chat_job_dispatches_to_chat_capable_worker() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::default();
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
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();

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

    // --- SessionRequested tests ---

    #[test]
    fn session_requested_adds_session() {
        // T1: SessionRequested adds a new session to sessions
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();

        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );

        assert_eq!(transition.state.sessions.len(), 1);
    }

    #[test]
    fn session_requested_starts_with_creator_subscribed() {
        // T2: New session starts with the creator in subscribers and empty entries
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();

        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );

        let (_, session_state) = transition.state.sessions.iter().next().unwrap();
        assert!(session_state.subscribers.contains_key(&s("sub-1")));
        assert_eq!(session_state.subscribers.len(), 1);
        assert!(session_state.entries.is_empty());
    }

    #[test]
    fn session_requested_deterministic_id() {
        // T3: Generated session ID is deterministic given runtime_id and session_counter
        let state_a: GatewayState<WId, SId, SubId> = GatewayState::default();
        let state_b: GatewayState<WId, SId, SubId> = GatewayState::default();

        let transition_a = reduce(
            state_a,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        let transition_b = reduce(
            state_b,
            Event::SessionRequested {
                subscriber_id: sub("sub-2"),
            },
        );

        // Same runtime_id (0) and session_counter (0) => same session ID
        let id_a: Vec<_> = transition_a.state.sessions.keys().collect();
        let id_b: Vec<_> = transition_b.state.sessions.keys().collect();
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn session_requested_increments_counter() {
        // T4: session_counter increments by 1
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();
        assert_eq!(state.session_counter, 0);

        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );

        assert_eq!(transition.state.session_counter, 1);
    }

    #[test]
    fn session_requested_emits_session_created() {
        // T5: Emits exactly one SessionCreated effect with generated session_id and subscriber_id
        let state: GatewayState<WId, SId, SubId> = GatewayState::default();

        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );

        assert_eq!(transition.effects.len(), 1);
        let session_id = transition.state.sessions.keys().next().unwrap().clone();
        assert_eq!(
            transition.effects[0],
            Effect::SessionCreated {
                session_id,
                subscriber_id: sub("sub-1"),
            }
        );
    }

    // T13: Tick propagates to sessions — stale subscriber removed
    #[test]
    fn t13_tick_propagates_removes_stale_subscriber() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 5);

        // Create a session (subscriber gets deadline = tick 0 + ttl 5 = 5)
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;

        // Tick past deadline
        for _ in 0..6 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        // Subscriber was removed by tick propagation, which left the session
        // with no subscribers, so the session itself was expired and removed.
        assert!(
            state.sessions.is_empty(),
            "session with stale subscriber should be expired and removed"
        );
    }

    // T14: Tick propagates to sessions — fresh subscriber kept
    #[test]
    fn t14_tick_propagates_keeps_fresh_subscriber() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 10);

        // Create a session (subscriber gets deadline = tick 0 + ttl 10 = 10)
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;

        // Tick but not past deadline
        for _ in 0..5 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        let (_, session_state) = state.sessions.iter().next().unwrap();
        assert!(
            session_state.subscribers.contains_key(&s("sub-1")),
            "fresh subscriber should still be present before deadline"
        );
    }

    // T15: SessionRequested sets subscriber_ttl and initial deadline on the creator
    #[test]
    fn t15_session_requested_sets_subscriber_ttl_and_deadline() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 7);

        // Advance tick to 3
        for _ in 0..3 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );

        let (_, session_state) = transition.state.sessions.iter().next().unwrap();
        assert_eq!(session_state.subscriber_ttl, 7);
        assert_eq!(
            session_state.subscribers.get(&s("sub-1")),
            Some(&10) // tick 3 + ttl 7
        );
    }

    // T16: Tick removes session with empty subscribers after tick propagation
    #[test]
    fn t16_tick_removes_session_with_empty_subscribers() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 5);

        // Create a session (subscriber deadline = tick 0 + ttl 5 = 5)
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;
        assert_eq!(state.sessions.len(), 1);

        // Tick past subscriber deadline so subscriber is removed, then session should expire
        for _ in 0..6 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        assert_eq!(
            state.sessions.len(),
            0,
            "session with no subscribers should be removed"
        );
    }

    // T17: Tick keeps session with remaining subscribers
    #[test]
    fn t17_tick_keeps_session_with_remaining_subscribers() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 10);

        // Create a session (subscriber deadline = tick 0 + ttl 10 = 10)
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;

        // Tick a few times but not past deadline
        for _ in 0..5 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        assert_eq!(
            state.sessions.len(),
            1,
            "session with active subscribers should not be removed"
        );
    }

    // T18: SessionExpired carries the full entry log
    #[test]
    fn t18_session_expired_carries_full_entry_log() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 5);

        // Create a session
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;
        let session_id = state.sessions.keys().next().cloned().unwrap();

        // Append entries
        let transition = reduce(
            state,
            Event::SessionEvent {
                session_id: session_id.clone(),
                event: session::Event::EntryAppended {
                    payload: json!({"msg": "hello"}),
                },
            },
        );
        state = transition.state;

        let transition = reduce(
            state,
            Event::SessionEvent {
                session_id: session_id.clone(),
                event: session::Event::EntryAppended {
                    payload: json!({"msg": "world"}),
                },
            },
        );
        state = transition.state;

        // Tick past subscriber deadline to trigger expiry
        for _ in 0..6 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        // The last tick should have produced a SessionExpired effect
        // Re-run the expiring tick to capture the effect
        // Actually, let's redo: we need to capture the transition that removes the session
        let mut state2: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 5);
        let transition = reduce(
            state2,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state2 = transition.state;
        let session_id = state2.sessions.keys().next().cloned().unwrap();

        let transition = reduce(
            state2,
            Event::SessionEvent {
                session_id: session_id.clone(),
                event: session::Event::EntryAppended {
                    payload: json!({"msg": "hello"}),
                },
            },
        );
        state2 = transition.state;

        let transition = reduce(
            state2,
            Event::SessionEvent {
                session_id: session_id.clone(),
                event: session::Event::EntryAppended {
                    payload: json!({"msg": "world"}),
                },
            },
        );
        state2 = transition.state;

        // Tick past deadline, checking each transition for the SessionExpired effect
        let mut found_expired = false;
        for _ in 0..6 {
            let transition = reduce(state2, Event::Tick);
            for effect in &transition.effects {
                if let Effect::SessionExpired {
                    session_id: sid,
                    entries,
                } = effect
                {
                    assert_eq!(sid, &session_id);
                    assert_eq!(entries.len(), 2);
                    assert_eq!(entries[0], json!({"msg": "hello"}));
                    assert_eq!(entries[1], json!({"msg": "world"}));
                    found_expired = true;
                }
            }
            state2 = transition.state;
        }

        assert!(found_expired, "expected SessionExpired effect with entries");
    }

    // T19: Multiple sessions expire in a single tick
    #[test]
    fn t19_multiple_sessions_expire_in_single_tick() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 3);

        // Create two sessions
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;

        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-2"),
            },
        );
        state = transition.state;
        assert_eq!(state.sessions.len(), 2);

        // Tick past both deadlines
        let mut expired_count = 0;
        for _ in 0..4 {
            let transition = reduce(state, Event::Tick);
            expired_count += transition
                .effects
                .iter()
                .filter(|e| matches!(e, Effect::SessionExpired { .. }))
                .count();
            state = transition.state;
        }

        assert_eq!(state.sessions.len(), 0, "both sessions should be removed");
        assert_eq!(expired_count, 2, "should have two SessionExpired effects");
    }

    // T20: Session with entries but no subscribers expires with entries preserved in effect
    #[test]
    fn t20_session_with_entries_expires_with_entries_in_effect() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 3);

        // Create session and add an entry
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;
        let session_id = state.sessions.keys().next().cloned().unwrap();

        let transition = reduce(
            state,
            Event::SessionEvent {
                session_id: session_id.clone(),
                event: session::Event::EntryAppended { payload: json!(42) },
            },
        );
        state = transition.state;

        // Tick past deadline
        let mut expired_entries = None;
        for _ in 0..4 {
            let transition = reduce(state, Event::Tick);
            for effect in &transition.effects {
                if let Effect::SessionExpired { entries, .. } = effect {
                    expired_entries = Some(entries.clone());
                }
            }
            state = transition.state;
        }

        let entries = expired_entries.expect("expected SessionExpired effect");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], json!(42));
    }

    // T21: Freshly created session (with creator subscribed) does not expire on same tick
    #[test]
    fn t21_freshly_created_session_does_not_expire_on_same_tick() {
        let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, 5);

        // Advance to tick 3
        for _ in 0..3 {
            let transition = reduce(state, Event::Tick);
            state = transition.state;
        }

        // Create session at tick 3 (subscriber deadline = 3 + 5 = 8)
        let transition = reduce(
            state,
            Event::SessionRequested {
                subscriber_id: sub("sub-1"),
            },
        );
        state = transition.state;

        // Tick once more (tick becomes 4, deadline is 8 — should NOT expire)
        let transition = reduce(state, Event::Tick);

        assert_eq!(
            transition.state.sessions.len(),
            1,
            "freshly created session must not expire on next tick"
        );
        let has_expired = transition
            .effects
            .iter()
            .any(|e| matches!(e, Effect::SessionExpired { .. }));
        assert!(
            !has_expired,
            "should not emit SessionExpired for fresh session"
        );
    }

    // --- Property tests ---

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        // Small ID pools to encourage collisions and interesting interactions
        const WORKER_IDS: &[&str] = &["w0", "w1", "w2"];
        const STREAM_IDS: &[&str] = &["s0", "s1", "s2"];
        const SUB_IDS: &[&str] = &["sub0", "sub1", "sub2"];
        const SESSION_IDS: &[&str] = &["sess0", "sess1"];

        fn arb_capability() -> impl Strategy<Value = Capability> {
            prop_oneof![Just(Capability::Chat), Just(Capability::Transcription),]
        }

        fn arb_capabilities() -> impl Strategy<Value = HashSet<Capability>> {
            proptest::collection::btree_set(arb_capability(), 1..=2)
                .prop_map(|s| s.into_iter().collect())
        }

        fn arb_event() -> impl Strategy<Value = Event<WId, SId, SubId>> {
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
                // SessionRequested
                prop::sample::select(SUB_IDS).prop_map(|sid| Event::SessionRequested {
                    subscriber_id: sub(sid),
                }),
                // SessionEvent
                (
                    prop::sample::select(SESSION_IDS),
                    prop_oneof![
                        Just(crate::gateway::session::Event::EntryAppended {
                            payload: json!({"data": "test"}),
                        }),
                        prop::sample::select(SUB_IDS).prop_map(|sub_id| {
                            crate::gateway::session::Event::Subscribed {
                                subscriber_id: sub(sub_id),
                                tick: 0,
                            }
                        }),
                        prop::sample::select(SUB_IDS).prop_map(|sub_id| {
                            crate::gateway::session::Event::Unsubscribed {
                                subscriber_id: sub(sub_id),
                            }
                        }),
                        (prop::sample::select(SUB_IDS).prop_map(sub), 0..100u64,).prop_map(
                            |(sub, tick)| {
                                crate::gateway::session::Event::SubscriberHeartbeat {
                                    subscriber_id: sub,
                                    tick,
                                }
                            }
                        ),
                        (0..100u64).prop_map(|tick| crate::gateway::session::Event::Tick { tick }),
                    ],
                )
                    .prop_map(|(sess, event)| Event::SessionEvent {
                        session_id: SessionId(String::from(sess)),
                        event,
                    }),
            ]
        }

        fn arb_event_sequence() -> impl Strategy<Value = Vec<Event<WId, SId, SubId>>> {
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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);
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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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

            // --- Session invariants ---

            #[test]
            fn session_counter_never_decreases(
                events in arb_event_sequence()
            ) {
                // I1: session_counter never decreases across any transition
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);
                let mut prev_counter = state.session_counter;

                for event in events {
                    let transition = reduce(state, event);
                    prop_assert!(
                        transition.state.session_counter >= prev_counter,
                        "session_counter went backwards: {} -> {}",
                        prev_counter,
                        transition.state.session_counter
                    );
                    prev_counter = transition.state.session_counter;
                    state = transition.state;
                }
            }

            #[test]
            fn sessions_only_removed_when_subscribers_empty(
                events in arb_event_sequence()
            ) {
                // P13: sessions are only removed when their subscriber set is empty
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

                for event in events {
                    let sessions_before = state.sessions.clone();

                    let transition = reduce(state, event);

                    // Any session that was removed must have had empty subscribers
                    // after tick propagation (i.e. in the post-transition check,
                    // any key present before but absent after must have had no subscribers)
                    for (sid, _session_state) in sessions_before.iter() {
                        if !transition.state.sessions.contains_key(sid) {
                            // Session was removed — verify it had no subscribers
                            // We check the pre-transition state's subscribers because
                            // the session was removed during this transition.
                            // However, tick propagation may have removed subscribers
                            // before the expiry check, so we verify via the effect:
                            // a SessionExpired effect must exist for this session.
                            let has_expired_effect = transition.effects.iter().any(|e| {
                                matches!(e, Effect::SessionExpired { session_id, .. } if session_id == sid)
                            });
                            prop_assert!(
                                has_expired_effect,
                                "session {:?} removed without SessionExpired effect",
                                sid
                            );
                        }
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn sessions_eventually_expire_without_heartbeats(
                initial_sessions in 1..5usize,
            ) {
                // P10: Every session eventually produces a SessionExpired effect
                // (given enough ticks without subscriber heartbeats)
                let ttl = 3u64;
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, ttl);

                // Create initial_sessions sessions
                for i in 0..initial_sessions {
                    let transition = reduce(
                        state,
                        Event::SessionRequested {
                            subscriber_id: sub(&format!("sub-{}", i)),
                        },
                    );
                    state = transition.state;
                }

                let session_ids: std::collections::BTreeSet<_> =
                    state.sessions.keys().cloned().collect();

                // Tick enough times for all to expire (ttl + 1 ticks should suffice)
                let mut expired_ids = std::collections::BTreeSet::new();
                for _ in 0..(ttl as usize + 2) {
                    let transition = reduce(state, Event::Tick);
                    for effect in &transition.effects {
                        if let Effect::SessionExpired { session_id, .. } = effect {
                            expired_ids.insert(session_id.clone());
                        }
                    }
                    state = transition.state;
                }

                prop_assert_eq!(
                    session_ids,
                    expired_ids,
                    "not all sessions produced SessionExpired"
                );
            }

            #[test]
            fn all_sessions_eventually_removed_without_heartbeats(
                initial_sessions in 1..5usize,
            ) {
                // P11: All sessions are eventually removed from state
                // (given enough ticks without subscriber heartbeats)
                let ttl = 3u64;
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, ttl);

                for i in 0..initial_sessions {
                    let transition = reduce(
                        state,
                        Event::SessionRequested {
                            subscriber_id: sub(&format!("sub-{}", i)),
                        },
                    );
                    state = transition.state;
                }

                prop_assert!(
                    !state.sessions.is_empty(),
                    "should have created sessions"
                );

                for _ in 0..(ttl as usize + 2) {
                    let transition = reduce(state, Event::Tick);
                    state = transition.state;
                }

                prop_assert!(
                    state.sessions.is_empty(),
                    "all sessions should be removed after enough ticks: {} remaining",
                    state.sessions.len()
                );
            }

            #[test]
            fn session_expired_carries_correct_entries(
                entry_count in 0..10usize,
            ) {
                // P12: SessionExpired effect carries the same entries that were
                // in the session at the time of removal
                let ttl = 5u64;
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 60, 30, ttl);

                let transition = reduce(
                    state,
                    Event::SessionRequested {
                        subscriber_id: sub("sub-1"),
                    },
                );
                state = transition.state;
                let session_id = state.sessions.keys().next().cloned().unwrap();

                // Append entry_count entries
                let mut expected_entries = Vec::new();
                for i in 0..entry_count {
                    let payload = json!({"idx": i});
                    expected_entries.push(payload.clone());
                    let transition = reduce(
                        state,
                        Event::SessionEvent {
                            session_id: session_id.clone(),
                            event: session::Event::EntryAppended { payload },
                        },
                    );
                    state = transition.state;
                }

                // Tick past deadline
                let mut expired_entries = None;
                for _ in 0..(ttl as usize + 2) {
                    let transition = reduce(state, Event::Tick);
                    for effect in &transition.effects {
                        if let Effect::SessionExpired { entries, .. } = effect {
                            expired_entries = Some(entries.clone());
                        }
                    }
                    state = transition.state;
                }

                let entries = expired_entries.expect("expected SessionExpired");
                prop_assert_eq!(
                    entries,
                    expected_entries,
                    "expired entries don't match appended entries"
                );
            }

            #[test]
            fn session_requested_only_modifies_sessions_and_counter(
                events in arb_event_sequence()
            ) {
                // I3: SessionRequested only modifies sessions and session_counter
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

                for event in events {
                    let is_session_requested = matches!(event, Event::SessionRequested { .. });

                    let tick_before = state.tick;
                    let available_before = state.available.clone();
                    let active_streams_before = state.active_streams.clone();
                    let worker_ttl_before = state.worker_ttl;
                    let stream_ttl_before = state.stream_ttl;
                    let runtime_id_before = state.runtime_id;

                    let transition = reduce(state, event);

                    if is_session_requested {
                        prop_assert!(
                            transition.state.tick == tick_before,
                            "SessionRequested modified tick"
                        );
                        prop_assert!(
                            transition.state.available == available_before,
                            "SessionRequested modified available"
                        );
                        prop_assert!(
                            transition.state.active_streams == active_streams_before,
                            "SessionRequested modified active_streams"
                        );
                        prop_assert!(
                            transition.state.worker_ttl == worker_ttl_before,
                            "SessionRequested modified worker_ttl"
                        );
                        prop_assert!(
                            transition.state.stream_ttl == stream_ttl_before,
                            "SessionRequested modified stream_ttl"
                        );
                        prop_assert!(
                            transition.state.runtime_id == runtime_id_before,
                            "SessionRequested modified runtime_id"
                        );
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn all_session_ids_are_unique(
                events in arb_event_sequence()
            ) {
                // I4: All session IDs produced across any event sequence are unique
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);
                let mut seen_ids: std::collections::BTreeSet<SessionId> =
                    std::collections::BTreeSet::new();

                for event in events {
                    let keys_before: std::collections::BTreeSet<_> =
                        state.sessions.keys().cloned().collect();

                    let transition = reduce(state, event);

                    let keys_after: std::collections::BTreeSet<_> =
                        transition.state.sessions.keys().cloned().collect();

                    let new_ids: Vec<_> = keys_after.difference(&keys_before).collect();
                    for id in new_ids {
                        prop_assert!(
                            seen_ids.insert(id.clone()),
                            "duplicate session ID: {}",
                            id
                        );
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn different_runtime_ids_produce_disjoint_session_ids(
                events in arb_event_sequence()
            ) {
                // I5: Two gateway states with different runtime_ids produce
                // strictly disjoint sets of session IDs for the same event sequence
                let mut state_a: GatewayState<WId, SId, SubId> = GatewayState::new(1, 3, 3, 30);
                let mut state_b: GatewayState<WId, SId, SubId> = GatewayState::new(2, 3, 3, 30);

                for event in events {
                    let transition_a = reduce(state_a, event.clone());
                    let transition_b = reduce(state_b, event);

                    state_a = transition_a.state;
                    state_b = transition_b.state;
                }

                let ids_a: std::collections::BTreeSet<_> =
                    state_a.sessions.keys().cloned().collect();
                let ids_b: std::collections::BTreeSet<_> =
                    state_b.sessions.keys().cloned().collect();

                let intersection: Vec<_> = ids_a.intersection(&ids_b).collect();
                prop_assert!(
                    intersection.is_empty(),
                    "runtime_id 1 and 2 produced overlapping session IDs: {:?}",
                    intersection
                );
            }

            // P9: Subscriber count across all sessions never increases from Tick
            #[test]
            fn p9_tick_never_increases_total_subscribers(
                events in arb_event_sequence()
            ) {
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

                for event in events {
                    let subs_before: usize = state
                        .sessions
                        .values()
                        .map(|s| s.subscribers.len())
                        .sum();
                    let transition = reduce(state, event.clone());

                    if matches!(event, Event::Tick) {
                        let subs_after: usize = transition
                            .state
                            .sessions
                            .values()
                            .map(|s| s.subscribers.len())
                            .sum();
                        prop_assert!(
                            subs_after <= subs_before,
                            "Tick increased total subscriber count from {} to {}",
                            subs_before, subs_after
                        );
                    }

                    state = transition.state;
                }
            }

            #[test]
            fn every_subscribed_sub_id_eventually_gets_removed(
                events in arb_event_sequence()
            ) {
                // P14: Every SubId that enters the kernel via SessionRequested
                // or SessionEvent::Subscribed eventually receives a
                // SubscriberRemoved effect (given enough ticks).
                // This covers both successful subscriptions and subscriptions
                // to non-existent sessions.
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);
                let mut entered_sub_ids: std::collections::BTreeSet<SubId> =
                    std::collections::BTreeSet::new();
                let mut removed_sub_ids: std::collections::BTreeSet<SubId> =
                    std::collections::BTreeSet::new();

                // Apply all events, tracking subscriber entries and removals
                for event in events {
                    // Track SubIds entering the kernel.
                    // Uses Event::sub_ids() which has no wildcard arm —
                    // adding a new event variant without updating sub_ids()
                    // is a compile error.
                    for sub_id in event.sub_ids() {
                        entered_sub_ids.insert(sub_id.clone());
                    }

                    let transition = reduce(state, event);

                    // Track SubscriberRemoved effects
                    for effect in &transition.effects {
                        if let Effect::SessionEffect(
                            crate::gateway::session::Effect::SubscriberRemoved {
                                subscriber_id,
                            },
                        ) = effect
                        {
                            removed_sub_ids.insert(subscriber_id.clone());
                        }
                    }

                    state = transition.state;
                }

                // Drain: apply enough ticks to expire all subscribers and sessions
                // subscriber_ttl is 30, so tick 31 should expire everything
                for tick in 0..35 {
                    let transition = reduce(state, Event::Tick);
                    for effect in &transition.effects {
                        if let Effect::SessionEffect(
                            crate::gateway::session::Effect::SubscriberRemoved {
                                subscriber_id,
                            },
                        ) = effect
                        {
                            removed_sub_ids.insert(subscriber_id.clone());
                        }
                    }
                    state = transition.state;

                    // Early exit if all accounted for
                    if entered_sub_ids.is_subset(&removed_sub_ids) {
                        break;
                    }
                    let _ = tick;
                }

                // Every SubId that entered must have been removed
                let missing: Vec<_> = entered_sub_ids
                    .difference(&removed_sub_ids)
                    .collect();
                prop_assert!(
                    missing.is_empty(),
                    "SubIds entered kernel but never received SubscriberRemoved: {:?}",
                    missing
                );
            }

            #[test]
            fn every_http_chat_requested_produces_terminal_effects(
                events in arb_event_sequence()
            ) {
                // Every HttpChatRequested produces terminal effects in the
                // same transition: either a DispatchJob (post-dispatch path,
                // where I2/I5 cover eventual termination) or
                // SendClientError + SendClientDone (pre-dispatch rejection).
                let mut state: GatewayState<WId, SId, SubId> = GatewayState::new(0, 3, 3, 30);

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
