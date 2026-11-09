//! In-process log bus.
//!
//! `tracing-subscriber` is not in the dependency tree, so the app keeps its own
//! ring buffer and broadcasts entries to whoever subscribed (the Tauri shell
//! forwards them to the UI; the CLI prints them).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use v2rayr_ipc::{LogEntry, LogLevel};

const CAPACITY: usize = 2_000;

pub struct LogBus {
    seq: AtomicU64,
    buffer: Mutex<VecDeque<LogEntry>>,
    sender: broadcast::Sender<LogEntry>,
}

impl LogBus {
    pub fn new() -> Arc<Self> {
        let (sender, _) = broadcast::channel(512);
        Arc::new(Self {
            seq: AtomicU64::new(1),
            buffer: Mutex::new(VecDeque::with_capacity(CAPACITY)),
            sender,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.sender.subscribe()
    }

    pub fn push(&self, level: LogLevel, target: impl Into<String>, message: impl Into<String>) {
        let entry = LogEntry {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            ts: chrono::Utc::now().timestamp_millis(),
            level,
            target: target.into(),
            message: message.into(),
        };
        {
            let mut buffer = self.buffer.lock().expect("log buffer");
            if buffer.len() == CAPACITY {
                buffer.pop_front();
            }
            buffer.push_back(entry.clone());
        }
        // Nobody listening is fine.
        let _ = self.sender.send(entry);
    }

    pub fn info(&self, target: impl Into<String>, message: impl Into<String>) {
        self.push(LogLevel::Info, target, message);
    }

    pub fn warn(&self, target: impl Into<String>, message: impl Into<String>) {
        self.push(LogLevel::Warn, target, message);
    }

    pub fn error(&self, target: impl Into<String>, message: impl Into<String>) {
        self.push(LogLevel::Error, target, message);
    }

    /// Newest `limit` entries, oldest first.
    pub fn tail(&self, limit: usize) -> Vec<LogEntry> {
        let buffer = self.buffer.lock().expect("log buffer");
        let skip = buffer.len().saturating_sub(limit);
        buffer.iter().skip(skip).cloned().collect()
    }

    pub fn since(&self, seq: u64, limit: usize) -> Vec<LogEntry> {
        let buffer = self.buffer.lock().expect("log buffer");
        buffer
            .iter()
            .filter(|entry| entry.seq > seq)
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_numbered_and_tailable() {
        let bus = LogBus::new();
        bus.info("test", "first");
        bus.warn("test", "second");
        let tail = bus.tail(10);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].seq, 1);
        assert_eq!(tail[1].level, LogLevel::Warn);
        assert_eq!(bus.since(1, 10).len(), 1);
    }

    #[test]
    fn the_buffer_is_bounded() {
        let bus = LogBus::new();
        for index in 0..(CAPACITY + 25) {
            bus.info("test", format!("line {index}"));
        }
        assert_eq!(bus.tail(usize::MAX).len(), CAPACITY);
    }

    #[tokio::test]
    async fn subscribers_receive_new_entries() {
        let bus = LogBus::new();
        let mut receiver = bus.subscribe();
        bus.info("test", "hello");
        let entry = receiver.recv().await.expect("entry");
        assert_eq!(entry.message, "hello");
    }
}
