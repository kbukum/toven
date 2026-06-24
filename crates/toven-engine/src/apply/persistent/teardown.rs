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
        self.process.shutdown()
    }

    fn health(&self) -> Health {
        Health::healthy(self.process.unit_id())
    }
}
