//! The shared argv→[`ProcessSpec`] lowering every runner shape composes.
//!
//! The synchronous one-shot [`ProcessToolRunner`](super::ProcessToolRunner) and
//! the async streaming [`ProcessCommandRunner`](super::ProcessCommandRunner)
//! legitimately differ in their trait surface and exit classification, but the
//! spec materialization underneath them is not shape-specific. This module is
//! the single home for it: the argv `split_first` guard, the
//! [`InvocationEnvPolicy`] → [`EnvPolicy`] mapping, explicit-variable
//! application, and (for the tool shape) working-directory and named-secret
//! forwarding. Both runners call in here so the guard and mapping exist in
//! exactly one place; future runners inherit the same policy for free.

use rskit_errors::{AppError, AppResult};
use rskit_process::{CapturedIo, EnvPolicy, InputPolicy, ProcessConfig, ProcessIo, ProcessSpec};
use toven_ports::{InvocationEnvPolicy, InvocationEnvironment, ToolInvocation};

/// Lower a fully-resolved `argv` + environment policy into an [`ProcessSpec`].
///
/// The single argv-guard (`argv[0]` is the program; an empty argv is a typed
/// [`invalid_input`](AppError::invalid_input) error tagged `argv_field`) and
/// env-policy mapping both runner shapes share. Callers layer their
/// shape-specific extras (working directory, secret forwarding, IO) on top.
///
/// # Errors
/// Returns [`AppError::invalid_input`] when `argv` is empty.
pub fn base_spec(
    argv: &[String],
    environment: &InvocationEnvironment,
    argv_field: &str,
) -> AppResult<ProcessSpec> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| AppError::invalid_input(argv_field, "must include a program"))?;
    Ok(ProcessSpec::new(program)
        .args(rest.iter().cloned())
        .env_policy(env_policy(environment.policy))
        .envs(environment.vars.clone()))
}

/// Map the port's environment policy onto the rskit-process policy.
const fn env_policy(policy: InvocationEnvPolicy) -> EnvPolicy {
    match policy {
        InvocationEnvPolicy::ExplicitOnly => EnvPolicy::Empty,
        InvocationEnvPolicy::InheritParent => EnvPolicy::Inherit,
    }
}

/// Lower a one-shot [`ToolInvocation`] into its [`ProcessSpec`].
///
/// The shared [`base_spec`] plus the tool-shape extras: the invocation's
/// working directory and named-secret forwarding. Each forwarded name is
/// resolved from the ambient environment at run time via
/// [`rskit_util::env::get_non_empty`]; an unset or empty name is skipped rather
/// than forwarded blank, and no value is ever placed on argv.
///
/// # Errors
/// Returns [`AppError::invalid_input`] when the invocation's argv is empty.
pub fn tool_spec(invocation: &ToolInvocation) -> AppResult<ProcessSpec> {
    let mut spec = base_spec(&invocation.argv, &invocation.environment, "tool.argv")?;
    if let Some(dir) = invocation.working_dir() {
        spec = spec.dir(dir);
    }
    for name in &invocation.forward_env {
        if let Some(value) = rskit_util::env::get_non_empty(name) {
            spec = spec.env(name.clone(), value);
        }
    }
    Ok(spec)
}

/// The captured, bounded [`ProcessConfig`] for a one-shot tool invocation.
///
/// `with_timeout(invocation.timeout)` clears rskit-process's inherited 30s
/// default so an invocation without a declared bound runs unbounded; a declared
/// bound is honored. Captured output stays bounded by the runner default cap
/// unless the invocation narrows it further.
#[must_use]
pub fn tool_config(invocation: &ToolInvocation) -> ProcessConfig {
    let mut captured = CapturedIo::new();
    if let Some(stdin) = &invocation.stdin {
        captured = captured.with_input(InputPolicy::Bytes(stdin.clone()));
    }
    let mut config = ProcessConfig::default()
        .with_io(ProcessIo::captured(captured))
        .with_timeout(invocation.timeout);
    if let Some(max_output_bytes) = invocation.max_output_bytes {
        config = config.with_max_output_bytes(max_output_bytes);
    }
    config
}
