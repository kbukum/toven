//! Managed federation smoke: drive the **real** `toven-go` driver binary in
//! `__serve` mode over real subprocess stdio.
//!
//! Unlike the engine's in-process [`ServeDouble`] round-trip (which exercises
//! the framed transport over OS pipes on a thread), this spawns the actual
//! shipping `toven-go` binary via `<program> __serve` and connects a
//! [`RemoteAdapter`](toven_core::federation::RemoteAdapter) to it exactly as
//! the umbrella would. It proves the argv-only spawn + handshake + prefetch
//! surface work end to end against a real process. It deliberately does **not**
//! call `discover` (which would shell out to the `go` toolchain), so the smoke
//! stays deterministic and toolchain-independent.

use std::path::PathBuf;

use toven_core::federation::RemoteAdapter;
use toven_model::EcosystemId;
use toven_ports::{ConfiguredAdapter, TaskKind};

/// Path to the freshly-built `toven-go` driver binary under test.
fn driver_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_toven-go"))
}

#[test]
fn umbrella_drives_the_real_go_driver_over_stdio() {
    let ecosystem = EcosystemId::new("go").expect("valid id");

    // Spawn `toven-go __serve` and complete the handshake + infallible prefetch
    // against the real process — no in-proc double, no shell string.
    let config: rskit_config::RawValue =
        rskit_codec::decode(&rskit_codec::TomlCodec, "modules = []").expect("subtree parses");
    let remote = RemoteAdapter::spawn(&driver_binary(), ecosystem, config)
        .expect("real toven-go __serve handshake + prefetch succeed");

    // The driver advertises a real toolchain probe from the prefetch handshake.
    let probe = remote.toolchain_probe();
    assert_eq!(probe.program, "go", "the go driver probes the go toolchain");

    // A run-strategy query is answered across recognized task kinds.
    let _ = remote.run_strategy_default(TaskKind::Build);
    let _ = remote.run_strategy_default(TaskKind::Test);

    // Dropping the adapter sends a graceful Shutdown and reaps the child.
    drop(remote);
}
