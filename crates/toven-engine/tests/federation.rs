//! Federation integration tests: the umbrella-side transport against an
//! in-process `__serve` double, plus the four-way driver dispatch semantics.
//!
//! No real subprocess is spawned for the transport tests: the
//! `ServeDouble` runs the engine's [`serve`](toven_engine::federation::serve)
//! loop on a thread connected by OS
//! pipes, so a [`RemoteAdapter`](toven_engine::federation::RemoteAdapter)
//! round-trips exactly as it would over a child's stdio. The dispatch tests drive
//! the real spawn path with a *bogus* pinned driver to prove a resolved-but-broken
//! driver is a hard error while an absent one warns and skips.

use std::collections::BTreeSet;
use std::io::{PipeReader, PipeWriter};
use std::thread::{self, JoinHandle};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_engine::config::{CanonicalRegistry, load};
use toven_engine::federation::RemoteAdapter;
use toven_engine::federation::protocol::{
    Capabilities, ENVELOPE_SCHEMA_VERSION, Hello, MAX_FRAME_BYTES, Response, ScaffoldOutcome,
    ScaffoldRequest, Welcome, read_value, write_value,
};
use toven_engine::federation::resolve::{PathDriverLocator, resolve_adapters};
use toven_engine::plan::dependency_graph;
use toven_model::{AbsPath, EcosystemId, Event};
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, FanOut, Provider, RunStrategy, Task,
    TaskKind, ToolchainProbe,
};
use toven_testkit::{FakeConfiguredAdapter, FakeProvider, RecordingReporter, fixtures};

/// A valid ecosystem id for tests.
fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

/// The scripted adapter a driven `go` server answers with: a non-default run
/// strategy, one task, and a recognizable probe, so the round-trip is observable.
fn scripted_go_adapter() -> FakeConfiguredAdapter {
    FakeConfiguredAdapter::new(eid("go"))
        .with_run_strategy(RunStrategy::Unordered)
        .with_tasks(vec![Task::new(
            TaskKind::Build,
            vec!["go".into(), "build".into()],
            FanOut::WholeWorkspace,
        )])
        .with_probe(ToolchainProbe::new("go", "go", vec!["version".into()]))
}

/// Load a config fixture into a strict `Document`, treating `loaded` as the
/// in-proc ecosystems.
fn load_document(rel: &str, loaded: &[&str]) -> toven_engine::config::Document {
    let path = fixtures::document_path(rel).expect("fixture path");
    let loaded_ids: BTreeSet<EcosystemId> = loaded.iter().map(|id| eid(id)).collect();
    load(&path, &loaded_ids, &CanonicalRegistry::model())
        .expect("fixture loads")
        .document
}

#[test]
fn remote_adapter_round_trips_the_port_surface_over_the_serve_double() {
    let ServeDouble {
        reader,
        writer,
        join,
    } = ServeDouble::spawn(|| {
        vec![Box::new(
            FakeProvider::new(eid("go")).with_adapter(scripted_go_adapter()),
        )]
    })
    .expect("serve double spawns");

    let remote = RemoteAdapter::connect_io(reader, writer, eid("go"), "modules = []".to_string())
        .expect("handshake + prefetch succeed");

    // The prefetched infallible surface mirrors the scripted adapter.
    let tasks = remote.default_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].kind, TaskKind::Build);
    assert_eq!(remote.toolchain_probe().program, "go");
    assert_eq!(
        remote.run_strategy_default(&TaskKind::Build),
        RunStrategy::Unordered
    );
    assert_eq!(
        remote.run_strategy_default(&TaskKind::Custom("e2e".into())),
        RunStrategy::Unordered
    );

    // The live `discover` RPC round-trips and is stamped with the request schema.
    let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute"));
    let response = remote.discover(&request).expect("discover round-trips");
    assert_eq!(response.ecosystem, eid("go"));
    assert_eq!(response.schema_version, request.schema_version);

    // Dropping the adapter sends a graceful Shutdown; the server loop ends Ok.
    drop(remote);
    join.wait().expect("serve loop exits cleanly");
}

#[test]
fn remote_adapter_mirrors_per_custom_name_run_strategies() {
    // A driver whose `run_strategy_default` varies by custom task name must be
    // mirrored faithfully: the umbrella prefetches each custom name declared in
    // `default_tasks`, so a declared name keeps its own strategy while an
    // undeclared one falls back to the generic custom default.
    let ServeDouble {
        reader,
        writer,
        join,
    } = ServeDouble::spawn(|| {
        let adapter = FakeConfiguredAdapter::new(eid("go"))
            .with_run_strategy(RunStrategy::LeafToTop)
            .with_tasks(vec![Task::new(
                TaskKind::Custom("e2e".into()),
                vec!["go".into(), "test".into()],
                FanOut::WholeWorkspace,
            )])
            .with_custom_run_strategy("e2e", RunStrategy::Unordered);
        vec![Box::new(FakeProvider::new(eid("go")).with_adapter(adapter))]
    })
    .expect("serve double spawns");

    let remote = RemoteAdapter::connect_io(reader, writer, eid("go"), "modules = []".to_string())
        .expect("handshake + prefetch succeed");

    // The declared custom task keeps the driver's per-name strategy...
    assert_eq!(
        remote.run_strategy_default(&TaskKind::Custom("e2e".into())),
        RunStrategy::Unordered
    );
    // ...while an undeclared custom name falls back to the generic default.
    assert_eq!(
        remote.run_strategy_default(&TaskKind::Custom("undeclared".into())),
        RunStrategy::LeafToTop
    );

    drop(remote);
    join.wait().expect("serve loop exits cleanly");
}

#[test]
fn fallback_custom_strategy_is_isolated_from_a_sentinel_named_task() {
    // The fallback for an *undeclared* custom task is prefetched with an internal
    // sentinel name. A driver that happens to declare a real custom task must not
    // poison that fallback even if its name resembles a probe sentinel: the
    // declared task keeps its own strategy while undeclared names fall back to the
    // driver's generic default, never the declared task's strategy.
    let ServeDouble {
        reader,
        writer,
        join,
    } = ServeDouble::spawn(|| {
        let adapter = FakeConfiguredAdapter::new(eid("go"))
            .with_run_strategy(RunStrategy::LeafToTop)
            .with_tasks(vec![Task::new(
                TaskKind::Custom("__probe__".into()),
                vec!["go".into(), "test".into()],
                FanOut::WholeWorkspace,
            )])
            .with_custom_run_strategy("__probe__", RunStrategy::Unordered);
        vec![Box::new(FakeProvider::new(eid("go")).with_adapter(adapter))]
    })
    .expect("serve double spawns");

    let remote = RemoteAdapter::connect_io(reader, writer, eid("go"), "modules = []".to_string())
        .expect("handshake + prefetch succeed");

    // The declared "__probe__" task keeps its own per-name strategy...
    assert_eq!(
        remote.run_strategy_default(&TaskKind::Custom("__probe__".into())),
        RunStrategy::Unordered
    );
    // ...and an undeclared custom name falls back to the generic default, NOT the
    // declared "__probe__" task's strategy (which a colliding sentinel would leak).
    assert_eq!(
        remote.run_strategy_default(&TaskKind::Custom("undeclared".into())),
        RunStrategy::LeafToTop
    );

    drop(remote);
    join.wait().expect("serve loop exits cleanly");
}

#[test]
fn handshake_accepts_an_additive_minor_protocol() {
    let ServeDouble {
        mut reader,
        mut writer,
        join,
    } = ServeDouble::spawn(|| vec![Box::new(FakeProvider::new(eid("go")))])
        .expect("serve double spawns");

    // A higher MINOR within the same MAJOR is compatible (additive).
    let hello = Hello {
        schema_version: ENVELOPE_SCHEMA_VERSION,
        protocol: "1.4.2".to_string(),
        ecosystem: eid("go"),
        config_toml: "modules = []".to_string(),
    };
    write_value(&mut writer, &hello).expect("send hello");
    let welcome: Welcome = read_value(&mut reader, MAX_FRAME_BYTES)
        .expect("read welcome")
        .expect("welcome present");
    assert_eq!(welcome.schema_version, ENVELOPE_SCHEMA_VERSION);

    drop(writer);
    drop(reader);
    join.wait().expect("serve loop exits cleanly");
}

#[test]
fn handshake_rejects_an_incompatible_major_protocol() {
    let ServeDouble {
        mut reader,
        mut writer,
        join,
    } = ServeDouble::spawn(|| vec![Box::new(FakeProvider::new(eid("go")))])
        .expect("serve double spawns");

    let hello = Hello {
        schema_version: ENVELOPE_SCHEMA_VERSION,
        protocol: "2.0.0".to_string(),
        ecosystem: eid("go"),
        config_toml: "modules = []".to_string(),
    };
    write_value(&mut writer, &hello).expect("send hello");

    // The server reports the incompatibility as a typed wire error before exiting.
    let response: Response = read_value(&mut reader, MAX_FRAME_BYTES)
        .expect("read response")
        .expect("response present");
    assert!(matches!(response, Response::Error(_)), "got {response:?}");

    drop(writer);
    drop(reader);
    // A rejected handshake is a hard error on the server side.
    assert!(
        join.wait().is_err(),
        "incompatible handshake must fail serve"
    );
}

#[test]
fn serve_rejects_an_ecosystem_it_does_not_serve() {
    let ServeDouble {
        mut reader,
        mut writer,
        join,
    } = ServeDouble::spawn(|| vec![Box::new(FakeProvider::new(eid("go")))])
        .expect("serve double spawns");

    // The server only serves `go`; asking it to act as `rust` is a hard error.
    let hello = Hello::new(
        "1.0.0".to_string(),
        eid("rust"),
        "manifests = []".to_string(),
    );
    write_value(&mut writer, &hello).expect("send hello");
    let response: Response = read_value(&mut reader, MAX_FRAME_BYTES)
        .expect("read response")
        .expect("response present");
    assert!(matches!(response, Response::Error(_)), "got {response:?}");

    drop(writer);
    drop(reader);
    assert!(join.wait().is_err(), "unknown ecosystem must fail serve");
}

#[test]
fn remote_adapter_surfaces_a_rejected_handshake_as_a_typed_remote_error() {
    // The server only serves `go`. Connecting a `rust` umbrella adapter must fail
    // the handshake with the *remote's* typed classification (NotFound) instead of
    // an opaque transport error, proving the client decodes an error reply frame.
    let ServeDouble {
        reader,
        writer,
        join,
    } = ServeDouble::spawn(|| vec![Box::new(FakeProvider::new(eid("go")))])
        .expect("serve double spawns");

    let Err(error) =
        RemoteAdapter::connect_io(reader, writer, eid("rust"), "manifests = []".to_string())
    else {
        panic!("handshake against an unserved ecosystem must fail")
    };
    assert_eq!(
        error.code(),
        ErrorCode::NotFound,
        "remote rejection keeps its typed code: {error}"
    );

    assert!(join.wait().is_err(), "unknown ecosystem must fail serve");
}

#[test]
fn serve_rejects_a_mismatched_envelope_schema() {
    let ServeDouble {
        mut reader,
        mut writer,
        join,
    } = ServeDouble::spawn(|| vec![Box::new(FakeProvider::new(eid("go")))])
        .expect("serve double spawns");

    // A hello carrying a future, breaking envelope schema must be rejected up
    // front (a Conflict) rather than parsed against the current wire shape.
    let hello = Hello {
        schema_version: ENVELOPE_SCHEMA_VERSION + 1,
        protocol: "1.0.0".to_string(),
        ecosystem: eid("go"),
        config_toml: "modules = []".to_string(),
    };
    write_value(&mut writer, &hello).expect("send hello");
    let response: Response = read_value(&mut reader, MAX_FRAME_BYTES)
        .expect("read response")
        .expect("response present");
    assert!(matches!(response, Response::Error(_)), "got {response:?}");

    drop(writer);
    drop(reader);
    assert!(
        join.wait().is_err(),
        "a mismatched envelope schema must fail serve"
    );
}

#[test]
fn remote_adapter_rejects_a_mismatched_welcome_schema() {
    // A driver that answers the handshake with a future envelope schema must be
    // rejected by the umbrella before any port call, even if it otherwise looks
    // well-formed. A minimal mock server emits exactly that welcome.
    let (umbrella_reader, driver_out) = std::io::pipe().expect("pipe");
    let (driver_in, umbrella_writer) = std::io::pipe().expect("pipe");

    let join: JoinHandle<()> = thread::spawn(move || {
        let mut reader = driver_in;
        let mut writer = driver_out;
        let _hello: Option<Hello> = read_value(&mut reader, MAX_FRAME_BYTES).expect("read hello");
        let welcome = Welcome {
            schema_version: ENVELOPE_SCHEMA_VERSION + 1,
            protocol: "1.0.0".to_string(),
            capabilities: Capabilities::plan_surface(),
            common: CommonEcosystemConfig::default(),
        };
        write_value(&mut writer, &welcome).expect("send welcome");
    });

    let Err(error) = RemoteAdapter::connect_io(
        umbrella_reader,
        umbrella_writer,
        eid("go"),
        "modules = []".to_string(),
    ) else {
        panic!("a mismatched welcome schema must fail the umbrella")
    };
    assert_eq!(error.code(), ErrorCode::Conflict, "got {error}");

    join.join().expect("mock driver thread exits");
}

#[test]
fn remote_adapter_rejects_a_driver_missing_required_capabilities() {
    // A driver that completes the handshake but does not advertise the required
    // PLAN surface (here: no `discover`) must be rejected up front as an
    // incompatible driver, before any port call is issued.
    let (umbrella_reader, driver_out) = std::io::pipe().expect("pipe");
    let (driver_in, umbrella_writer) = std::io::pipe().expect("pipe");

    let join: JoinHandle<()> = thread::spawn(move || {
        let mut reader = driver_in;
        let mut writer = driver_out;
        let _hello: Option<Hello> = read_value(&mut reader, MAX_FRAME_BYTES).expect("read hello");
        let mut capabilities = Capabilities::plan_surface();
        capabilities.discover = false;
        let welcome = Welcome {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            protocol: "1.0.0".to_string(),
            capabilities,
            common: CommonEcosystemConfig::default(),
        };
        write_value(&mut writer, &welcome).expect("send welcome");
    });

    let Err(error) = RemoteAdapter::connect_io(
        umbrella_reader,
        umbrella_writer,
        eid("go"),
        "modules = []".to_string(),
    ) else {
        panic!("a driver missing required capabilities must fail the umbrella")
    };
    assert_eq!(error.code(), ErrorCode::Conflict, "got {error}");
    assert!(
        error.to_string().contains("discover"),
        "error should name the missing capability: {error}"
    );

    join.join().expect("mock driver thread exits");
}

#[test]
fn resolved_but_broken_driver_is_a_hard_plan_error() {
    // `[ecosystems.go].driver` pins a non-existent binary; resolving it must fail
    // the PLAN rather than silently skip the ecosystem.
    let document = load_document("valid/driver-pin-bogus.toml", &["rust"]);
    let rust = FakeProvider::new(eid("rust"));
    let providers: Vec<&dyn Provider> = vec![&rust];

    let result = resolve_adapters(&document, &providers, &PathDriverLocator::new());
    assert!(
        result.is_err(),
        "a resolved-but-broken pinned driver must hard-error"
    );
}

/// A locator that never resolves a driver on PATH.
struct NoLocator;
impl toven_engine::federation::resolve::DriverLocator for NoLocator {
    fn locate(&self, _binary_name: &str) -> Option<std::path::PathBuf> {
        None
    }
}

#[test]
fn absent_driver_warns_and_skips() {
    // `[ecosystems.go]` is canonical-but-unloaded with no pin and no PATH driver:
    // it must warn and skip (no adapter, no error).
    let document = load_document("valid/canonical-unloaded.toml", &["rust"]);
    let rust = FakeProvider::new(eid("rust"));
    let providers: Vec<&dyn Provider> = vec![&rust];

    let resolution =
        resolve_adapters(&document, &providers, &NoLocator).expect("absent driver does not error");
    assert!(
        resolution.adapters.is_empty(),
        "no remote adapter is connected for an absent driver"
    );
    assert!(
        resolution
            .warnings
            .iter()
            .any(|warning| warning.contains("go")),
        "an absent canonical ecosystem produces an actionable warning: {:?}",
        resolution.warnings
    );
}

#[test]
fn absent_driver_warning_is_surfaced_through_the_plan_front() {
    // The data-layer warning above is only useful if a user observes it. Drive
    // the same absent-driver document through the public PLAN front and assert
    // the skip reaches the reporter as an `Event::Warning` (not silently dropped).
    let document = load_document("valid/canonical-unloaded.toml", &["rust"]);
    let rust = FakeProvider::new(eid("rust")).with_adapter(FakeConfiguredAdapter::new(eid("rust")));
    let providers: Vec<&dyn Provider> = vec![&rust];

    let mut reporter = RecordingReporter::new();
    dependency_graph(
        &AbsPath::new("/repo").expect("absolute"),
        &document,
        &providers,
        &NoLocator,
        &mut reporter,
    )
    .expect("graph builds with the absent driver skipped");

    assert!(
        reporter.events().iter().any(|event| matches!(
            event,
            Event::Warning { message } if message.contains("go")
        )),
        "an absent driver must surface a warning event: {:?}",
        reporter.events()
    );
}

struct ServeDouble {
    reader: PipeReader,
    writer: PipeWriter,
    join: ServeJoin,
}

struct ServeJoin {
    handle: JoinHandle<AppResult<()>>,
}

impl ServeJoin {
    fn wait(self) -> AppResult<()> {
        self.handle
            .join()
            .map_err(|_| AppError::new(ErrorCode::Internal, "__serve double thread panicked"))?
    }
}

impl ServeDouble {
    fn spawn<F>(build: F) -> AppResult<Self>
    where
        F: FnOnce() -> Vec<Box<dyn Provider>> + Send + 'static,
    {
        let (driver_in, umbrella_writer) = std::io::pipe().map_err(|e| pipe_error(&e))?;
        let (umbrella_reader, driver_out) = std::io::pipe().map_err(|e| pipe_error(&e))?;

        let handle = thread::spawn(move || {
            let providers = build();
            let refs: Vec<&dyn Provider> = providers.iter().map(Box::as_ref).collect();
            toven_engine::federation::serve(&refs, driver_in, driver_out)
        });

        Ok(Self {
            reader: umbrella_reader,
            writer: umbrella_writer,
            join: ServeJoin { handle },
        })
    }
}

fn pipe_error(error: &std::io::Error) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!("could not create __serve double pipe: {error}"),
    )
}

#[test]
fn scaffold_exchange_round_trips_over_the_framed_transport() {
    // Drive the real config-less scaffold wire (serve_scaffold ↔ probe_io) over
    // OS pipes — no subprocess — exactly as the federated `toven generate` probe
    // would over a `toven-<eco> __scaffold` child's stdio.
    let (umbrella_reader, driver_out) = std::io::pipe().expect("pipe");
    let (driver_in, umbrella_writer) = std::io::pipe().expect("pipe");

    let mut table = toml::Table::new();
    table.insert(
        "manifests".to_string(),
        toml::Value::Array(vec![toml::Value::String("go.mod".to_string())]),
    );
    let scripted = toven_ports::EcosystemFragment::new(eid("go"), table);

    let driver_fragment = scripted.clone();
    let join: JoinHandle<()> = thread::spawn(move || {
        let provider = FakeProvider::new(eid("go")).with_scaffold(Some(driver_fragment));
        let providers: Vec<&dyn Provider> = vec![&provider];
        toven_engine::federation::serve_scaffold(&providers, driver_in, driver_out)
            .expect("serve_scaffold completes");
    });

    let fragments = toven_engine::federation::probe_io(
        umbrella_reader,
        umbrella_writer,
        "toven-go",
        std::path::Path::new("/repo"),
    )
    .expect("scaffold probe succeeds");

    assert_eq!(fragments, vec![scripted]);
    join.join().expect("driver scaffold thread exits");
}

#[test]
fn scaffold_driver_failure_surfaces_as_a_typed_error() {
    // A driver whose own self-detection errors must reach the umbrella as a typed
    // failure (serve_scaffold → ScaffoldOutcome::Error → probe_io decode), never a
    // silent empty fragment set.
    let (umbrella_reader, driver_out) = std::io::pipe().expect("pipe");
    let (driver_in, umbrella_writer) = std::io::pipe().expect("pipe");

    let join: JoinHandle<()> = thread::spawn(move || {
        let provider = FakeProvider::new(eid("go"))
            .with_scaffold_error(ErrorCode::InvalidInput, "go.mod is malformed");
        let providers: Vec<&dyn Provider> = vec![&provider];
        toven_engine::federation::serve_scaffold(&providers, driver_in, driver_out)
            .expect("serve_scaffold completes the exchange even when detection fails");
    });

    let error = toven_engine::federation::probe_io(
        umbrella_reader,
        umbrella_writer,
        "toven-go",
        std::path::Path::new("/repo"),
    )
    .expect_err("a driver detection failure must be a hard error");

    assert_eq!(
        error.code(),
        ErrorCode::InvalidInput,
        "the driver's typed code must survive the wire round-trip"
    );
    assert!(
        error.message().contains("go.mod is malformed"),
        "{}",
        error.message()
    );
    join.join().expect("driver scaffold thread exits");
}

#[test]
fn scaffold_schema_mismatch_is_reported_as_a_typed_error() {
    // An umbrella speaking a future envelope schema must get a typed Conflict
    // reply, not a misparsed fragment set.
    let (umbrella_reader, driver_out) = std::io::pipe().expect("pipe");
    let (driver_in, umbrella_writer) = std::io::pipe().expect("pipe");

    let join: JoinHandle<()> = thread::spawn(move || {
        let provider = FakeProvider::new(eid("go"));
        let providers: Vec<&dyn Provider> = vec![&provider];
        toven_engine::federation::serve_scaffold(&providers, driver_in, driver_out)
            .expect("serve_scaffold replies to a version-skewed request");
    });

    let mut writer = umbrella_writer;
    let mut reader = umbrella_reader;
    let request = ScaffoldRequest {
        schema_version: ENVELOPE_SCHEMA_VERSION + 1,
        project_root: std::path::PathBuf::from("/repo"),
    };
    write_value(&mut writer, &request).expect("send a version-skewed request");

    let outcome = read_value::<_, ScaffoldOutcome>(&mut reader, MAX_FRAME_BYTES)
        .expect("read the reply")
        .expect("driver replies before closing");

    match outcome {
        ScaffoldOutcome::Error(wire) => {
            assert_eq!(wire.code, ErrorCode::Conflict.as_str());
            assert!(wire.message.contains("envelope schema"), "{}", wire.message);
        }
        ScaffoldOutcome::Fragments(fragments) => {
            panic!("a schema mismatch must not yield fragments: {fragments:?}")
        }
        other => panic!("expected a typed Error reply, got {other:?}"),
    }
    join.join().expect("driver scaffold thread exits");
}

#[test]
fn scaffold_peer_closing_before_a_request_is_a_clean_no_op() {
    // If the umbrella drops the stream before sending a ScaffoldRequest, the
    // driven `serve_scaffold` loop must exit Ok(()) without writing a reply —
    // never an error and never a stray frame.
    let (umbrella_reader, driver_out) = std::io::pipe().expect("pipe");
    let (driver_in, umbrella_writer) = std::io::pipe().expect("pipe");

    // Close the umbrella's write end immediately so the driver reads EOF.
    drop(umbrella_writer);

    let join: JoinHandle<AppResult<()>> = thread::spawn(move || {
        let provider = FakeProvider::new(eid("go"));
        let providers: Vec<&dyn Provider> = vec![&provider];
        toven_engine::federation::serve_scaffold(&providers, driver_in, driver_out)
    });

    let mut reader = umbrella_reader;
    let reply = read_value::<_, ScaffoldOutcome>(&mut reader, MAX_FRAME_BYTES)
        .expect("reading the closed driver stream is not an error");
    assert!(
        reply.is_none(),
        "a peer that closes before requesting must get no reply frame, got {reply:?}"
    );

    join.join()
        .expect("driver scaffold thread exits")
        .expect("serve_scaffold treats an early EOF as a clean no-op");
}
