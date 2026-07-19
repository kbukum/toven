//! rskit-component-backed LIFO teardown backstop for held persistent units.

use std::sync::Arc;

use async_trait::async_trait;
use rskit_component::{Component, Health, Registry};
use rskit_errors::AppResult;

use super::held::SharedHeldProcess;

/// Reverse-order teardown registry for persistent processes.
pub(super) struct TeardownRegistry {
    registry: Registry,
}

impl TeardownRegistry {
    /// Create an empty registry.
    #[must_use]
    pub(super) fn new() -> Self {
        Self {
            registry: Registry::new(),
        }
    }

    /// Register a held process for the eventual LIFO backstop.
    pub(super) fn register(&mut self, process: SharedHeldProcess) {
        self.registry
            .register(Arc::new(HeldComponent { process }) as Arc<dyn Component>);
    }

    /// Run the reverse-order backstop.
    pub(super) async fn stop_all(&self) -> AppResult<()> {
        self.registry.start_all().await?;
        self.registry.stop_all().await
    }
}

struct HeldComponent {
    process: SharedHeldProcess,
}

#[async_trait]
impl Component for HeldComponent {
    fn name(&self) -> &str {
        self.process.unit_id()
    }

    async fn start(&self) -> AppResult<()> {
        Ok(())
    }

    async fn stop(&self) -> AppResult<()> {
        // Offload the synchronous, potentially process-join-blocking shutdown to a
        // blocking thread so the async teardown caller (e.g. `teardown_held`) can keep
        // draining the bounded live-output bridge while shutdown waits. Running
        // shutdown inline here would park this task on the runtime and stall draining,
        // deadlocking a reader thread parked in `blocking_send`.
        self.process.clone().shutdown_offloaded().await
    }

    fn health(&self) -> Health {
        Health::healthy(self.process.unit_id())
    }
}
