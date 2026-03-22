use std::collections::HashMap;
use std::fmt;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::protocol::Capability;

use super::relay::StreamFrame;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientStreamId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriberId(String);

pub type WorkerHandle = oneshot::Sender<WorkerJob>;
pub type StreamHandle = mpsc::Sender<StreamFrame>;

// pub trait SendWorkerJob {
//     fn send(self, job: WorkerJob) -> Result<(), WorkerJob>;
// }

// pub struct OpaqueWorkerHandle<T>(pub T);

// impl<T: SendWorkerJob> OpaqueWorkerHandle<T> {
//     pub fn send(self, job: WorkerJob) -> Result<(), WorkerJob> {
//         self.0.send(job)
//     }
// }

// impl SendWorkerJob for WorkerHandle {
//     fn send(self, job: WorkerJob) -> Result<(), WorkerJob> {
//         self.send(job)
//     }
// }

#[derive(Debug)]
pub struct WorkerJob {
    pub client_stream_id: ClientStreamId,
    pub capability: Capability,
    pub payload: Value,
    pub client_tx: StreamHandle,
}

impl WorkerId {
    fn new(value: String) -> Self {
        Self(value)
    }
}

impl ClientStreamId {
    fn new(value: String) -> Self {
        Self(value)
    }
}

impl SubscriberId {
    fn new(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for ClientStreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for SubscriberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug)]
pub struct ChannelRegistry<WorkerHandle, StreamHandle, SubscriberHandle> {
    workers: HashMap<WorkerId, WorkerHandle>,
    streams: HashMap<ClientStreamId, StreamHandle>,
    subscribers: HashMap<SubscriberId, SubscriberHandle>,
}

impl<WorkerHandle, StreamHandle, SubscriberHandle> Default
    for ChannelRegistry<WorkerHandle, StreamHandle, SubscriberHandle>
{
    fn default() -> Self {
        Self {
            workers: HashMap::new(),
            streams: HashMap::new(),
            subscribers: HashMap::new(),
        }
    }
}

impl<WorkerHandle, StreamHandle, SubscriberHandle>
    ChannelRegistry<WorkerHandle, StreamHandle, SubscriberHandle>
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_worker(&mut self, handle: WorkerHandle) -> WorkerId {
        let worker_id = WorkerId::new(Uuid::new_v4().to_string());
        self.workers.insert(worker_id.clone(), handle);
        worker_id
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Remove and return the worker handle, consuming it.
    /// Used for oneshot dispatch — the handle is gone after this.
    pub fn take_worker(&mut self, worker_id: &WorkerId) -> Option<WorkerHandle> {
        self.workers.remove(worker_id)
    }

    pub fn clone_stream(&self, stream_id: &ClientStreamId) -> Option<StreamHandle>
    where
        StreamHandle: Clone,
    {
        self.streams.get(stream_id).cloned()
    }

    /// Remove and return the stream handle, consuming it.
    /// Used for terminal effects — the handle is gone after this,
    /// which closes the channel if no other senders remain.
    pub fn take_stream(&mut self, stream_id: &ClientStreamId) -> Option<StreamHandle> {
        self.streams.remove(stream_id)
    }

    pub fn register_stream(&mut self, handle: StreamHandle) -> ClientStreamId {
        let stream_id = ClientStreamId::new(Uuid::new_v4().to_string());
        self.streams.insert(stream_id.clone(), handle);
        stream_id
    }

    pub fn register_subscriber(&mut self, handle: SubscriberHandle) -> SubscriberId {
        let subscriber_id = SubscriberId::new(Uuid::new_v4().to_string());
        self.subscribers.insert(subscriber_id.clone(), handle);
        subscriber_id
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    pub fn clone_subscriber(&self, subscriber_id: &SubscriberId) -> Option<SubscriberHandle>
    where
        SubscriberHandle: Clone,
    {
        self.subscribers.get(subscriber_id).cloned()
    }

    /// Remove and return the subscriber handle, consuming it.
    /// Used for terminal effects — the handle is gone after this,
    /// which closes the channel if no other senders remain.
    pub fn take_subscriber(&mut self, subscriber_id: &SubscriberId) -> Option<SubscriberHandle> {
        self.subscribers.remove(subscriber_id)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn register_worker_returns_unique_ids() {
        let mut registry = ChannelRegistry::<u8, (), ()>::new();

        let first = registry.register_worker(1u8);
        let second = registry.register_worker(2u8);

        assert_ne!(first, second);
        assert_eq!(registry.worker_count(), 2);
    }

    #[test]
    fn take_worker_removes_handle() {
        let mut registry = ChannelRegistry::<u8, (), ()>::new();
        let worker_id = registry.register_worker(42u8);

        assert_eq!(registry.take_worker(&worker_id), Some(42u8));
        assert_eq!(registry.take_worker(&worker_id), None);
        assert_eq!(registry.worker_count(), 0);
    }

    #[test]
    fn clone_stream_returns_handle_without_removing() {
        let mut registry = ChannelRegistry::<(), u8, ()>::new();
        let stream_id = registry.register_stream(42u8);

        assert_eq!(registry.clone_stream(&stream_id), Some(42u8));
        // Still in registry
        assert_eq!(registry.clone_stream(&stream_id), Some(42u8));
    }

    #[test]
    fn clone_stream_returns_none_for_unknown() {
        let registry = ChannelRegistry::<(), u8, ()>::new();
        let fake_id = ClientStreamId::new("nonexistent".to_string());
        assert_eq!(registry.clone_stream(&fake_id), None);
    }

    #[test]
    fn take_stream_removes_handle() {
        let mut registry = ChannelRegistry::<(), u8, ()>::new();
        let stream_id = registry.register_stream(42u8);

        assert_eq!(registry.take_stream(&stream_id), Some(42u8));
        assert_eq!(registry.take_stream(&stream_id), None);
        // Also gone from clone path
        assert_eq!(registry.clone_stream(&stream_id), None);
    }

    #[test]
    fn take_stream_returns_none_for_unknown() {
        let mut registry = ChannelRegistry::<(), u8, ()>::new();
        let fake_id = ClientStreamId::new("nonexistent".to_string());
        assert_eq!(registry.take_stream(&fake_id), None);
    }

    #[test]
    fn register_subscriber_returns_unique_ids() {
        let mut registry = ChannelRegistry::<(), (), u8>::new();

        let first = registry.register_subscriber(1u8);
        let second = registry.register_subscriber(2u8);

        assert_ne!(first, second);
        assert_eq!(registry.subscriber_count(), 2);
    }

    #[test]
    fn clone_subscriber_returns_handle_without_removing() {
        let mut registry = ChannelRegistry::<(), (), u8>::new();
        let sub_id = registry.register_subscriber(42u8);

        assert_eq!(registry.clone_subscriber(&sub_id), Some(42u8));
        // Still in registry
        assert_eq!(registry.clone_subscriber(&sub_id), Some(42u8));
    }

    #[test]
    fn clone_subscriber_returns_none_for_unknown() {
        let registry = ChannelRegistry::<(), (), u8>::new();
        let fake_id = SubscriberId::new("nonexistent".to_string());
        assert_eq!(registry.clone_subscriber(&fake_id), None);
    }

    #[test]
    fn take_subscriber_removes_handle() {
        let mut registry = ChannelRegistry::<(), (), u8>::new();
        let sub_id = registry.register_subscriber(42u8);

        assert_eq!(registry.take_subscriber(&sub_id), Some(42u8));
        assert_eq!(registry.take_subscriber(&sub_id), None);
        assert_eq!(registry.clone_subscriber(&sub_id), None);
    }

    #[test]
    fn take_subscriber_returns_none_for_unknown() {
        let mut registry = ChannelRegistry::<(), (), u8>::new();
        let fake_id = SubscriberId::new("nonexistent".to_string());
        assert_eq!(registry.take_subscriber(&fake_id), None);
    }

    proptest! {
        #[test]
        fn take_returns_registered_handle_for_each_id(handles in proptest::collection::vec(any::<u32>(), 0..128)) {
            let mut registry = ChannelRegistry::<u32, (), ()>::new();
            let mut registrations: Vec<(WorkerId, u32)> = Vec::new();

            for handle in handles {
                let worker_id = registry.register_worker(handle);
                registrations.push((worker_id, handle));
            }

            for (worker_id, expected_handle) in registrations {
                prop_assert_eq!(registry.take_worker(&worker_id), Some(expected_handle));
            }

            prop_assert_eq!(registry.worker_count(), 0);
        }
    }
}
