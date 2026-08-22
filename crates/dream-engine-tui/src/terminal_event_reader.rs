use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::{self, Event};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(25);

pub(super) struct TerminalEventReader {
    receiver: UnboundedReceiver<io::Result<Event>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TerminalEventReader {
    pub(super) fn new() -> Self {
        let (sender, receiver) = unbounded_channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown.clone();
        let worker = thread::spawn(move || {
            while !worker_shutdown.load(Ordering::Acquire) {
                match event::poll(EVENT_POLL_INTERVAL) {
                    Ok(true) => {
                        if sender.send(event::read()).is_err() {
                            break;
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        Self {
            receiver,
            shutdown,
            worker: Some(worker),
        }
    }

    pub(super) async fn next(&mut self) -> Option<io::Result<Event>> {
        self.receiver.recv().await
    }

    pub(super) fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    pub(super) fn restart(&mut self) {
        *self = Self::new();
    }
}

impl Drop for TerminalEventReader {
    fn drop(&mut self) {
        self.stop();
    }
}
