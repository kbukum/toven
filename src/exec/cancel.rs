//! Shared cancellation helpers for execution orchestration.
#![allow(clippy::redundant_pub_crate)]

use std::thread;

use tokio_util::sync::CancellationToken;

use crate::core::{AppError, AppResult, ErrorCode};

#[derive(Debug, Clone)]
pub(crate) struct SharedCancellation {
    token: CancellationToken,
}

impl SharedCancellation {
    pub(crate) fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    pub(crate) fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub(crate) fn cancel(&self) {
        self.token.cancel();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

pub(crate) struct CtrlCHandler {
    stop: CancellationToken,
    thread: thread::JoinHandle<AppResult<()>>,
}

pub(crate) fn spawn_ctrl_c_handler(cancel: SharedCancellation) -> AppResult<CtrlCHandler> {
    spawn_ctrl_c_handler_with_notify(cancel, || {})
}

pub(crate) fn spawn_ctrl_c_handler_with_notify<F>(
    cancel: SharedCancellation,
    notify: F,
) -> AppResult<CtrlCHandler>
where
    F: FnOnce() + Send + 'static,
{
    let stop = CancellationToken::new();
    let wait_stop = stop.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to create ctrl-c runtime").with_cause(error)
        })?;
    let thread = thread::spawn(move || {
        runtime.block_on(async move {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.map_err(|error| {
                        AppError::new(ErrorCode::Internal, "failed to listen for ctrl-c").with_cause(error)
                    })?;
                    cancel.cancel();
                    notify();
                    Ok(())
                }
                () = wait_stop.cancelled() => Ok(()),
            }
        })
    });
    Ok(CtrlCHandler { stop, thread })
}

pub(crate) fn stop_ctrl_c_handler(handler: Option<CtrlCHandler>) -> AppResult<()> {
    if let Some(handler) = handler {
        handler.stop.cancel();
        handler
            .thread
            .join()
            .map_err(|_| AppError::new(ErrorCode::Internal, "ctrl-c handler panicked"))?
    } else {
        Ok(())
    }
}
