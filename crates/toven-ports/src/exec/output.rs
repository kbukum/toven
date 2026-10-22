//! Live output observer passed to command runners.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rskit_errors::{AppError, AppResult, ErrorCode};
use tokio::sync::mpsc::Sender;
use toven_model::UnitOutput;

/// Handle onto the bounded live-output bridge that carries output while an
/// invocation is still running.
///
/// Delivery is lossless: [`emit`](Self::emit) waits for bridge capacity, so a
/// slow consumer applies backpressure down to the child's pipe instead of
/// losing chunks. A chunk is only counted as dropped when the consumer has gone
/// away and it can no longer be delivered.
#[derive(Clone, Default)]
pub struct OutputObserver {
    sender: Option<Sender<UnitOutput>>,
    dropped: DroppedOutput,
    terminal: bool,
}

impl OutputObserver {
    /// Create an observer with no consumer; emitted output is discarded.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Create an observer that delivers output into `sender`.
    #[must_use]
    pub fn new(sender: Sender<UnitOutput>) -> Self {
        Self {
            sender: Some(sender),
            dropped: DroppedOutput::default(),
            terminal: false,
        }
    }

    /// Allow terminal rendering because this observer's output is displayed live.
    #[must_use]
    pub const fn with_terminal(mut self) -> Self {
        self.terminal = true;
        self
    }

    /// Whether the consumer displays output live and can accept terminal rendering.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Shared count of chunks that could not be delivered.
    #[must_use]
    pub fn dropped(&self) -> DroppedOutput {
        self.dropped.clone()
    }

    /// Emit one chunk from async code, waiting for bridge capacity.
    ///
    /// # Errors
    /// Returns an error when the configured consumer has closed.
    pub async fn emit(&self, chunk: UnitOutput) -> AppResult<()> {
        if let Some(sender) = &self.sender
            && let Err(error) = sender.send(chunk).await
        {
            self.dropped.record();
            return Err(
                AppError::new(ErrorCode::Internal, "live output consumer closed").with_cause(error),
            );
        }
        Ok(())
    }

    /// Emit one chunk from a dedicated OS thread, blocking for bridge capacity.
    ///
    /// # Errors
    ///
    /// Returns an error inside a Tokio runtime, where callers must use [`emit`](Self::emit), or when the configured consumer has closed.
    pub fn emit_blocking(&self, chunk: UnitOutput) -> AppResult<()> {
        if let Some(sender) = &self.sender {
            if tokio::runtime::Handle::try_current().is_ok() {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    "blocking output delivery requires a dedicated thread outside a Tokio runtime; use emit().await",
                ));
            }
            if let Err(error) = sender.blocking_send(chunk) {
                self.dropped.record();
                return Err(
                    AppError::new(ErrorCode::Internal, "live output consumer closed")
                        .with_cause(error),
                );
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for OutputObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutputObserver")
            .field("configured", &self.sender.is_some())
            .field("terminal", &self.terminal)
            .field("dropped", &self.dropped.count())
            .finish()
    }
}

/// Count of live-output chunks that could not be delivered because the
/// consumer had gone away.
#[derive(Clone, Debug, Default)]
pub struct DroppedOutput(Arc<AtomicUsize>);

impl DroppedOutput {
    /// Number of chunks dropped so far.
    #[must_use]
    pub fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    fn record(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toven_model::OutputStream;

    fn chunk(byte: u8) -> UnitOutput {
        UnitOutput {
            unit_id: "unit".to_string(),
            stream: OutputStream::Stdout,
            bytes: vec![byte],
        }
    }

    #[tokio::test]
    async fn emit_waits_for_capacity_instead_of_dropping() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let observer = OutputObserver::new(sender);
        let producer = {
            let observer = observer.clone();
            tokio::spawn(async move {
                for byte in 0..10 {
                    observer.emit(chunk(byte)).await.expect("delivery");
                }
            })
        };
        let mut delivered = Vec::new();
        while delivered.len() < 10 {
            delivered.push(receiver.recv().await.expect("chunk").bytes[0]);
        }
        producer.await.expect("producer");
        assert_eq!(delivered, (0..10).collect::<Vec<_>>());
        assert_eq!(observer.dropped().count(), 0);
    }

    #[tokio::test]
    async fn emit_counts_chunks_once_the_consumer_is_gone() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let observer = OutputObserver::new(sender);
        assert!(observer.emit(chunk(1)).await.is_err());
        let blocking = observer.clone();
        std::thread::spawn(move || blocking.emit_blocking(chunk(2)))
            .join()
            .expect("thread")
            .expect_err("closed consumer");
        assert_eq!(observer.dropped().count(), 2);
    }

    #[tokio::test]
    async fn none_discards_without_counting() {
        let observer = OutputObserver::none();
        observer.emit(chunk(1)).await.expect("discard");
        assert_eq!(observer.dropped().count(), 0);
    }

    #[tokio::test]
    async fn blocking_emit_rejects_runtime_callers_without_panicking() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let observer = OutputObserver::new(sender);
        let error = observer.emit_blocking(chunk(1)).expect_err("async context");
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(observer.dropped().count(), 0);
        assert!(receiver.try_recv().is_err());
        observer.emit(chunk(2)).await.expect("async delivery");
        assert_eq!(receiver.recv().await.expect("chunk").bytes, [2]);
    }
}
