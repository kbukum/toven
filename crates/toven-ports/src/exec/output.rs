//! Live output observer passed to command runners.

use std::sync::Arc;

use toven_model::UnitOutput;

/// Callback sink for output that must be routed while an invocation is still running.
#[derive(Clone, Default)]
pub struct OutputObserver {
    emit: Option<Arc<dyn Fn(UnitOutput) + Send + Sync + 'static>>,
}

impl OutputObserver {
    /// Create an observer that drops no output because nothing is registered.
    #[must_use]
    pub const fn none() -> Self {
        Self { emit: None }
    }

    /// Create an observer from an infallible output callback.
    #[must_use]
    pub fn new(callback: impl Fn(UnitOutput) + Send + Sync + 'static) -> Self {
        Self {
            emit: Some(Arc::new(callback)),
        }
    }

    /// Emit one live output chunk.
    pub fn emit(&self, chunk: UnitOutput) {
        if let Some(emit) = &self.emit {
            emit(chunk);
        }
    }
}

impl std::fmt::Debug for OutputObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutputObserver")
            .field("configured", &self.emit.is_some())
            .finish()
    }
}
