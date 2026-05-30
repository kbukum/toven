use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use tokio_util::sync::CancellationToken;

use crate::core::{AppError, AppResult, ErrorCode};

pub(super) struct CtrlCHandler {
    token: CancellationToken,
    thread: thread::JoinHandle<AppResult<()>>,
}

pub(super) fn spawn_ctrl_c_handler(
    cancel_token: CancellationToken,
    cancelled: Arc<AtomicBool>,
) -> AppResult<CtrlCHandler> {
    let token = CancellationToken::new();
    let wait_token = token.clone();
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
                    cancelled.store(true, Ordering::SeqCst);
                    cancel_token.cancel();
                    Ok(())
                }
                () = wait_token.cancelled() => Ok(()),
            }
        })
    });
    Ok(CtrlCHandler { token, thread })
}

pub(super) fn stop_ctrl_c_handler(handler: Option<CtrlCHandler>) -> AppResult<()> {
    if let Some(handler) = handler {
        handler.token.cancel();
        handler
            .thread
            .join()
            .map_err(|_| AppError::new(ErrorCode::Internal, "persistent ctrl-c handler panicked"))?
    } else {
        Ok(())
    }
}
