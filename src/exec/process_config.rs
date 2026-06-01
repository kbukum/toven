//! Explicit process I/O configuration helpers.

use std::time::Duration;

use rskit_process::{
    CapturedIo, InputPolicy, ObservedIo, OutputObserver, OutputPolicy, ProcessConfig, ProcessIo,
};

pub(crate) fn captured_config(
    timeout: Option<Duration>,
    input: InputPolicy,
    output: OutputPolicy,
) -> ProcessConfig {
    ProcessConfig::default()
        .with_timeout(timeout)
        .with_io(ProcessIo::captured(
            CapturedIo::new().with_input(input).with_output(output),
        ))
}

pub(crate) fn observed_config(
    timeout: Option<Duration>,
    input: InputPolicy,
    output: OutputPolicy,
    observer: OutputObserver,
) -> ProcessConfig {
    ProcessConfig::default()
        .with_timeout(timeout)
        .with_io(ProcessIo::observed(
            ObservedIo::new(observer)
                .with_input(input)
                .with_output(output),
        ))
}
