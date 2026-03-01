pub mod effect;
pub mod event;
pub mod job;
pub mod state;
pub mod worker_table;

use effect::Effect;
use event::Event;
use state::State;
use tokio::sync::mpsc::Receiver;
use worker_table::WorkerRecord;

pub struct Engine {
    event_receiver: Receiver<Event>,
    state: State,
}

impl Engine {
    pub fn init(event_receiver: Receiver<Event>) -> Self {
        Self {
            event_receiver,
            state: State::new(),
        }
    }

    pub async fn run<Fut>(self, effect_handler: impl Fn(Effect) -> Fut)
    where
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut engine = self;
        while let Some(event) = engine.event_receiver.recv().await {
            let (new_state, effect) = handle_event(engine.state, event);
            engine.state = new_state;
            effect_handler(effect).await;
        }
    }
}

pub fn handle_event(state: State, event: Event) -> (State, Effect) {
    match event {
        Event::WorkerConnected {
            worker_id,
            capabilities,
            instant,
        } => {
            let State { worker_table } = state;
            let worker_record = WorkerRecord {
                capabilities,
                last_heartbeat_received_at: instant,
            };
            (
                State {
                    worker_table: worker_table.with_worker_added(worker_id, worker_record),
                },
                Effect::None,
            )
        }

        Event::WorkerHeartbeatReceived { worker_id, instant } => {
            let State { worker_table } = state;
            (
                State {
                    worker_table: worker_table.with_worker_heartbeat_updated(&worker_id, instant),
                },
                Effect::None,
            )
        }

        Event::JobRequested { job } => (state, Effect::None),
    }
}
