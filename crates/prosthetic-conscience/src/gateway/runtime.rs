use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::gateway::channel_registry::WorkerHandle;

use super::channel_registry::{
    ChannelRegistry, ClientStreamId, StreamHandle, SubscriberHandle, SubscriberId, WorkerId,
};
use super::effects::dispatch_job::DispatchJob;
use super::effects::send_client_done::SendClientDone;
use super::effects::send_client_error::SendClientError;
use crate::protocol::SessionGatewayMessage;

use super::kernel::Capability;
use super::kernel::{Effect, Event, GatewayState, SessionId, Transition, reduce};
use super::session;

/// Configuration for the gateway runtime.
pub struct GatewayConfig {
    /// How often the runtime sends `Tick` events to the kernel.
    pub tick_interval: Duration,
    /// Ticks until an idle worker expires from the kernel.
    pub worker_ttl: u64,
    /// Ticks until an active stream expires (timeout).
    pub stream_ttl: u64,
    /// How often the relay sends `StreamHeartbeat` commands during active streaming.
    pub stream_heartbeat_interval: Duration,
    /// How often the worker WS handler sends `WorkerHeartbeat` commands while idle.
    pub worker_heartbeat_interval: Duration,
    /// Ticks until an idle session subscriber expires.
    pub subscriber_ttl: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_secs(1),
            worker_ttl: 60,
            stream_ttl: 30,
            stream_heartbeat_interval: Duration::from_secs(10),
            worker_heartbeat_interval: Duration::from_secs(15),
            subscriber_ttl: 30,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionEntriesQuery {
    pub entries: Vec<Value>,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct SessionSubscription {
    pub subscriber_id: SubscriberId,
    pub latest_entry_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub tick: u64,
    pub available_workers: usize,
    pub active_streams: usize,
    pub worker_registry_count: usize,
    pub stream_registry_count: usize,
    pub subscriber_registry_count: usize,
}

type KernelEvent = Event<WorkerId, ClientStreamId, SubscriberId>;
type KernelEffect = Effect<WorkerId, ClientStreamId, SubscriberId>;
type ResolvedSId = (ClientStreamId, StreamHandle);
type ResolvedSubId = (SubscriberId, SubscriberHandle);
type ResolvedEffect = Effect<WorkerHandle, ResolvedSId, ResolvedSubId>;

#[derive(Debug)]
pub enum RuntimeCommand {
    RegisterWorker {
        handle: WorkerHandle,
        capabilities: BTreeSet<Capability>,
        reply_tx: oneshot::Sender<WorkerId>,
    },
    WorkerHeartbeat {
        worker_id: WorkerId,
    },
    AssignmentCleared {
        client_stream_id: ClientStreamId,
    },
    AssignmentFailed {
        client_stream_id: ClientStreamId,
        message: String,
    },
    StreamHeartbeat {
        client_stream_id: ClientStreamId,
    },
    RegisterStream {
        handle: StreamHandle,
        reply_tx: oneshot::Sender<ClientStreamId>,
    },
    HttpChatRequested {
        client_stream_id: ClientStreamId,
        payload: Value,
        stream: bool,
        required_capability: Capability,
    },
    QueryState {
        reply_tx: oneshot::Sender<StateSnapshot>,
    },
    QuerySessionEntries {
        session_id: SessionId,
        from: usize,
        limit: usize,
        reply_tx: oneshot::Sender<Option<SessionEntriesQuery>>,
    },
    SessionCreate {
        handle: SubscriberHandle,
        reply_tx: oneshot::Sender<SubscriberId>,
    },
    SessionSubscribe {
        session_id: SessionId,
        handle: SubscriberHandle,
        reply_tx: oneshot::Sender<SessionSubscription>,
    },
    SessionAppendEntry {
        session_id: SessionId,
        payload: Value,
    },
    SessionSubscriberHeartbeat {
        session_id: SessionId,
        subscriber_id: SubscriberId,
    },
    SessionUnsubscribe {
        session_id: SessionId,
        subscriber_id: SubscriberId,
    },
}

enum RuntimeMessage {
    Command(RuntimeCommand),
    Event(KernelEvent),
}

#[derive(Clone)]
pub struct RuntimeHandle {
    msg_tx: mpsc::Sender<RuntimeMessage>,
    /// Relay uses this to schedule stream heartbeats during active streaming.
    pub stream_heartbeat_interval: Duration,
    /// Worker WS handler uses this to schedule idle heartbeats between jobs.
    pub worker_heartbeat_interval: Duration,
}

impl RuntimeHandle {
    pub async fn submit_command(&self, command: RuntimeCommand) -> Result<(), RuntimeSendError> {
        self.msg_tx
            .send(RuntimeMessage::Command(command))
            .await
            .map_err(|_| RuntimeSendError)
    }

    pub async fn register_worker(
        &self,
        handle: WorkerHandle,
        capabilities: BTreeSet<Capability>,
    ) -> Result<WorkerId, RegisterError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit_command(RuntimeCommand::RegisterWorker {
            handle,
            capabilities,
            reply_tx,
        })
        .await?;
        let worker_id = reply_rx.await?;
        Ok(worker_id)
    }

    pub async fn register_stream(
        &self,
        handle: StreamHandle,
    ) -> Result<ClientStreamId, RegisterError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit_command(RuntimeCommand::RegisterStream { handle, reply_tx })
            .await?;
        let stream_id = reply_rx.await?;
        Ok(stream_id)
    }

    pub async fn worker_heartbeat(&self, worker_id: WorkerId) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::WorkerHeartbeat { worker_id })
            .await
    }

    pub async fn stream_heartbeat(
        &self,
        client_stream_id: ClientStreamId,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::StreamHeartbeat { client_stream_id })
            .await
    }

    pub async fn assignment_cleared(
        &self,
        client_stream_id: ClientStreamId,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::AssignmentCleared { client_stream_id })
            .await
    }

    pub async fn assignment_failed(
        &self,
        client_stream_id: ClientStreamId,
        message: String,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::AssignmentFailed {
            client_stream_id,
            message,
        })
        .await
    }

    pub async fn http_chat_requested(
        &self,
        client_stream_id: ClientStreamId,
        payload: Value,
        stream: bool,
        required_capability: Capability,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::HttpChatRequested {
            client_stream_id,
            payload,
            stream,
            required_capability,
        })
        .await
    }

    pub async fn query_state(&self) -> Result<StateSnapshot, RuntimeSendError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit_command(RuntimeCommand::QueryState { reply_tx })
            .await?;
        reply_rx.await.map_err(|_| RuntimeSendError)
    }

    pub async fn query_session_entries(
        &self,
        session_id: SessionId,
        from: usize,
        limit: usize,
    ) -> Result<Option<SessionEntriesQuery>, RuntimeSendError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit_command(RuntimeCommand::QuerySessionEntries {
            session_id,
            from,
            limit,
            reply_tx,
        })
        .await?;
        reply_rx.await.map_err(|_| RuntimeSendError)
    }

    pub async fn session_create(
        &self,
        handle: SubscriberHandle,
    ) -> Result<SubscriberId, RegisterError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit_command(RuntimeCommand::SessionCreate { handle, reply_tx })
            .await?;
        let subscriber_id = reply_rx.await?;
        Ok(subscriber_id)
    }

    pub async fn session_subscribe(
        &self,
        session_id: SessionId,
        handle: SubscriberHandle,
    ) -> Result<SessionSubscription, RegisterError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit_command(RuntimeCommand::SessionSubscribe {
            session_id,
            handle,
            reply_tx,
        })
        .await?;
        let subscription = reply_rx.await?;
        Ok(subscription)
    }

    pub async fn session_append_entry(
        &self,
        session_id: SessionId,
        payload: Value,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::SessionAppendEntry {
            session_id,
            payload,
        })
        .await
    }

    pub async fn session_subscriber_heartbeat(
        &self,
        session_id: SessionId,
        subscriber_id: SubscriberId,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::SessionSubscriberHeartbeat {
            session_id,
            subscriber_id,
        })
        .await
    }

    pub async fn session_unsubscribe(
        &self,
        session_id: SessionId,
        subscriber_id: SubscriberId,
    ) -> Result<(), RuntimeSendError> {
        self.submit_command(RuntimeCommand::SessionUnsubscribe {
            session_id,
            subscriber_id,
        })
        .await
    }
}

#[derive(Debug, thiserror::Error)]
#[error("runtime channel closed")]
pub struct RuntimeSendError;

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("runtime channel closed")]
    Send(#[from] RuntimeSendError),
    #[error("runtime dropped register reply channel")]
    ReplyClosed(#[from] oneshot::error::RecvError),
}

pub struct GatewayRuntime {
    state: GatewayState<WorkerId, ClientStreamId, SubscriberId>,
    registry: ChannelRegistry<WorkerHandle, StreamHandle, SubscriberHandle>,
}

impl GatewayRuntime {
    fn apply_event(self, event: KernelEvent) -> (Self, Vec<KernelEffect>) {
        let Transition { state, effects } = reduce(self.state, event);
        (
            Self {
                state,
                registry: self.registry,
            },
            effects,
        )
    }

    pub fn handle_register_worker(
        mut self,
        worker_handle: WorkerHandle,
        capabilities: BTreeSet<Capability>,
        reply_tx: oneshot::Sender<WorkerId>,
    ) -> (Self, Vec<KernelEffect>) {
        let worker_id = self.registry.register_worker(worker_handle);
        // If the reply channel is dropped (caller task cancelled), the worker becomes
        // an orphan entry. This is benign: dispatch to it fails at oneshot send,
        // triggering recovery. Future heartbeat/timeout will clean it up. We intentionally
        // do not roll back the registration here to avoid coupling this path to
        // unregister semantics.
        let _ = reply_tx.send(worker_id.clone());

        self.apply_event(Event::WorkerRegistered {
            worker_id,
            capabilities: capabilities.into_iter().collect(),
        })
    }

    pub fn handle_worker_heartbeat(self, worker_id: WorkerId) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::WorkerHeartbeat { worker_id })
    }

    pub fn handle_assignment_cleared(
        self,
        client_stream_id: ClientStreamId,
    ) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::AssignmentCleared { client_stream_id })
    }

    pub fn handle_assignment_failed(
        self,
        client_stream_id: ClientStreamId,
        message: String,
    ) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::AssignmentFailed {
            client_stream_id,
            message,
        })
    }

    pub fn handle_stream_heartbeat(
        self,
        client_stream_id: ClientStreamId,
    ) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::StreamHeartbeat { client_stream_id })
    }

    pub fn handle_http_chat_requested(
        self,
        client_stream_id: ClientStreamId,
        payload: Value,
        stream: bool,
        required_capability: Capability,
    ) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::HttpChatRequested {
            client_stream_id,
            payload,
            stream,
            required_capability,
        })
    }

    pub fn handle_register_stream(
        mut self,
        stream_handle: StreamHandle,
        reply_tx: oneshot::Sender<ClientStreamId>,
    ) -> (Self, Vec<KernelEffect>) {
        let stream_id = self.registry.register_stream(stream_handle);
        let _ = reply_tx.send(stream_id);

        (self, Vec::new())
    }

    pub fn handle_session_create(
        mut self,
        handle: SubscriberHandle,
        reply_tx: oneshot::Sender<SubscriberId>,
    ) -> (Self, Vec<KernelEffect>) {
        let subscriber_id = self.registry.register_subscriber(handle);
        let _ = reply_tx.send(subscriber_id.clone());
        self.apply_event(Event::SessionRequested { subscriber_id })
    }

    pub fn handle_session_subscribe(
        mut self,
        session_id: SessionId,
        handle: SubscriberHandle,
        reply_tx: oneshot::Sender<SessionSubscription>,
    ) -> (Self, Vec<KernelEffect>) {
        let latest_entry_index = self
            .state
            .sessions
            .get(&session_id)
            .and_then(|session| session.entries.len().checked_sub(1));
        let subscriber_id = self.registry.register_subscriber(handle);
        let _ = reply_tx.send(SessionSubscription {
            subscriber_id: subscriber_id.clone(),
            latest_entry_index,
        });
        let tick = self.state.tick;
        self.apply_event(Event::SessionEvent {
            session_id,
            event: session::Event::Subscribed {
                subscriber_id,
                tick,
            },
        })
    }

    pub fn handle_session_append_entry(
        self,
        session_id: SessionId,
        payload: Value,
    ) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::SessionEvent {
            session_id,
            event: session::Event::EntryAppended { payload },
        })
    }

    pub fn handle_session_subscriber_heartbeat(
        self,
        session_id: SessionId,
        subscriber_id: SubscriberId,
    ) -> (Self, Vec<KernelEffect>) {
        let tick = self.state.tick;
        self.apply_event(Event::SessionEvent {
            session_id,
            event: session::Event::SubscriberHeartbeat {
                subscriber_id,
                tick,
            },
        })
    }

    pub fn handle_session_unsubscribe(
        self,
        session_id: SessionId,
        subscriber_id: SubscriberId,
    ) -> (Self, Vec<KernelEffect>) {
        self.apply_event(Event::SessionEvent {
            session_id,
            event: session::Event::Unsubscribed { subscriber_id },
        })
    }

    fn resolve_effects(
        &mut self,
        effects: Vec<KernelEffect>,
    ) -> (Vec<ResolvedEffect>, Vec<KernelEvent>) {
        let mut resolved = Vec::new();
        let mut fallbacks = Vec::new();

        for effect in effects {
            match effect {
                Effect::DispatchJob(e) => {
                    let DispatchJob {
                        worker_id,
                        client_stream_id,
                        capability,
                        payload,
                    } = e;
                    // Take the oneshot sender out of the registry (consumed on use)
                    let worker_handle = match self.registry.take_worker(&worker_id) {
                        Some(handle) => handle,
                        None => {
                            fallbacks.push(Event::AssignmentFailed {
                                client_stream_id,
                                message: String::from("worker handle not found"),
                            });
                            continue;
                        }
                    };
                    let stream_handle = match self.registry.clone_stream(&client_stream_id) {
                        Some(handle) => handle,
                        None => {
                            fallbacks.push(Event::AssignmentFailed {
                                client_stream_id,
                                message: String::from("stream handle not found"),
                            });
                            continue;
                        }
                    };
                    resolved.push(Effect::DispatchJob(DispatchJob {
                        worker_id: worker_handle,
                        client_stream_id: (client_stream_id, stream_handle),
                        capability,
                        payload,
                    }));
                }
                Effect::SendClientError(e) => {
                    if let Some(stream_handle) = self.registry.clone_stream(&e.client_stream_id) {
                        resolved.push(Effect::SendClientError(SendClientError {
                            client_stream_id: (e.client_stream_id, stream_handle),
                            message: e.message,
                        }));
                    }
                }
                Effect::SendClientDone(e) => {
                    if let Some(stream_handle) = self.registry.take_stream(&e.client_stream_id) {
                        resolved.push(Effect::SendClientDone(SendClientDone {
                            client_stream_id: (e.client_stream_id, stream_handle),
                        }));
                    }
                }
                Effect::ProtocolViolation(e) => resolved.push(Effect::ProtocolViolation(e)),
                Effect::SessionCreated {
                    session_id,
                    subscriber_id,
                } => {
                    if let Some(handle) = self.registry.clone_subscriber(&subscriber_id) {
                        resolved.push(Effect::SessionCreated {
                            session_id,
                            subscriber_id: (subscriber_id, handle),
                        });
                    }
                }
                Effect::SessionExpired {
                    session_id,
                    entries,
                } => {
                    resolved.push(Effect::SessionExpired {
                        session_id,
                        entries,
                    });
                }
                Effect::SessionEffect(e) => match e {
                    session::Effect::NotifySubscribers {
                        entry_index,
                        payload,
                        subscribers,
                    } => {
                        let resolved_subscribers: Vec<_> = subscribers
                            .into_iter()
                            .filter_map(|sid| {
                                self.registry
                                    .clone_subscriber(&sid)
                                    .map(|handle| (sid, handle))
                            })
                            .collect();
                        if !resolved_subscribers.is_empty() {
                            resolved.push(Effect::SessionEffect(
                                session::Effect::NotifySubscribers {
                                    entry_index,
                                    payload,
                                    subscribers: resolved_subscribers,
                                },
                            ));
                        }
                    }
                    session::Effect::SubscriberRemoved { subscriber_id } => {
                        if let Some(handle) = self.registry.take_subscriber(&subscriber_id) {
                            resolved.push(Effect::SessionEffect(
                                session::Effect::SubscriberRemoved {
                                    subscriber_id: (subscriber_id, handle),
                                },
                            ));
                        }
                    }
                },
            }
        }

        (resolved, fallbacks)
    }

    fn process_message(
        self,
        message: RuntimeMessage,
        msg_tx: &mpsc::Sender<RuntimeMessage>,
    ) -> (Self, Vec<ResolvedEffect>) {
        let (mut updated_runtime, effects) =
            match message {
                RuntimeMessage::Command(command) => match command {
                    RuntimeCommand::RegisterWorker {
                        handle,
                        capabilities,
                        reply_tx,
                    } => self.handle_register_worker(handle, capabilities, reply_tx),
                    RuntimeCommand::WorkerHeartbeat { worker_id } => {
                        self.handle_worker_heartbeat(worker_id)
                    }
                    RuntimeCommand::AssignmentCleared { client_stream_id } => {
                        self.handle_assignment_cleared(client_stream_id)
                    }
                    RuntimeCommand::AssignmentFailed {
                        client_stream_id,
                        message,
                    } => self.handle_assignment_failed(client_stream_id, message),
                    RuntimeCommand::StreamHeartbeat { client_stream_id } => {
                        self.handle_stream_heartbeat(client_stream_id)
                    }
                    RuntimeCommand::RegisterStream { handle, reply_tx } => {
                        self.handle_register_stream(handle, reply_tx)
                    }
                    RuntimeCommand::HttpChatRequested {
                        client_stream_id,
                        payload,
                        stream,
                        required_capability,
                    } => self.handle_http_chat_requested(
                        client_stream_id,
                        payload,
                        stream,
                        required_capability,
                    ),
                    RuntimeCommand::QueryState { reply_tx } => {
                        let snapshot = StateSnapshot {
                            tick: self.state.tick,
                            available_workers: self.state.available.len(),
                            active_streams: self.state.active_streams.len(),
                            worker_registry_count: self.registry.worker_count(),
                            stream_registry_count: self.registry.stream_count(),
                            subscriber_registry_count: self.registry.subscriber_count(),
                        };
                        let _ = reply_tx.send(snapshot);
                        (self, Vec::new())
                    }
                    RuntimeCommand::QuerySessionEntries {
                        session_id,
                        from,
                        limit,
                        reply_tx,
                    } => {
                        let result = self.state.sessions.get(&session_id).map(|session| {
                            SessionEntriesQuery {
                                entries: session.entries.slice(from, limit).to_vec(),
                                total: session.entries.len(),
                            }
                        });
                        let _ = reply_tx.send(result);
                        (self, Vec::new())
                    }
                    RuntimeCommand::SessionCreate { handle, reply_tx } => {
                        self.handle_session_create(handle, reply_tx)
                    }
                    RuntimeCommand::SessionSubscribe {
                        session_id,
                        handle,
                        reply_tx,
                    } => self.handle_session_subscribe(session_id, handle, reply_tx),
                    RuntimeCommand::SessionAppendEntry {
                        session_id,
                        payload,
                    } => self.handle_session_append_entry(session_id, payload),
                    RuntimeCommand::SessionSubscriberHeartbeat {
                        session_id,
                        subscriber_id,
                    } => self.handle_session_subscriber_heartbeat(session_id, subscriber_id),
                    RuntimeCommand::SessionUnsubscribe {
                        session_id,
                        subscriber_id,
                    } => self.handle_session_unsubscribe(session_id, subscriber_id),
                },
                RuntimeMessage::Event(event) => self.apply_event(event),
            };

        let (resolved_effects, fallback_events) = updated_runtime.resolve_effects(effects);

        if !fallback_events.is_empty() {
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                for event in fallback_events {
                    let _ = tx.send(RuntimeMessage::Event(event)).await;
                }
            });
        }

        (updated_runtime, resolved_effects)
    }

    pub fn spawn(config: GatewayConfig) -> RuntimeHandle {
        let mut runtime = GatewayRuntime {
            state: GatewayState::new(
                uuid::Uuid::new_v4().as_u128(),
                config.worker_ttl,
                config.stream_ttl,
                config.subscriber_ttl,
            ),
            registry: ChannelRegistry::new(),
        };

        let (msg_tx, mut msg_rx) = mpsc::channel::<RuntimeMessage>(256);
        let handle = RuntimeHandle {
            msg_tx: msg_tx.clone(),
            stream_heartbeat_interval: config.stream_heartbeat_interval,
            worker_heartbeat_interval: config.worker_heartbeat_interval,
        };
        let effect_tx = msg_tx.clone();
        let effect_handle = handle.clone();

        // Tick task: sends Tick events on a timer, skipping when channel is congested.
        let tick_tx = msg_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(config.tick_interval);
            loop {
                ticker.tick().await;
                match tick_tx.try_send(RuntimeMessage::Event(Event::Tick)) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {} // skip tick under congestion
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
        });

        tokio::spawn(async move {
            while let Some(message) = msg_rx.recv().await {
                let (updated_runtime, resolved_effects) =
                    runtime.process_message(message, &effect_tx);
                spawn_effects(resolved_effects, &effect_handle);
                runtime = updated_runtime;
            }
        });

        handle
    }
}

fn spawn_effects(effects: Vec<ResolvedEffect>, runtime: &RuntimeHandle) {
    if effects.is_empty() {
        return;
    }

    let runtime = runtime.clone();
    tokio::spawn(async move {
        for effect in effects {
            match effect {
                Effect::DispatchJob(e) => e.execute(&runtime).await,
                Effect::SendClientError(e) => e.execute().await,
                Effect::SendClientDone(e) => e.execute().await,
                Effect::ProtocolViolation(e) => e.execute().await,
                Effect::SessionCreated {
                    session_id,
                    subscriber_id: (_, handle),
                } => {
                    let _ = handle
                        .send(SessionGatewayMessage::Subscribed {
                            session_id: session_id.0,
                            latest_entry_index: None,
                        })
                        .await;
                }
                Effect::SessionEffect(session_effect) => match session_effect {
                    session::Effect::NotifySubscribers {
                        entry_index,
                        payload,
                        subscribers,
                    } => {
                        for (_, handle) in subscribers {
                            let _ = handle
                                .send(SessionGatewayMessage::Entry {
                                    index: entry_index,
                                    payload: payload.clone(),
                                })
                                .await;
                        }
                    }
                    session::Effect::SubscriberRemoved {
                        subscriber_id: (_, handle),
                    } => {
                        let _ = handle.send(SessionGatewayMessage::SubscriberRemoved).await;
                        // handle is dropped here, closing the channel
                    }
                },
                Effect::SessionExpired { .. } => {
                    // No-op until persistence adapters are wired
                }
            }
        }
    });
}
