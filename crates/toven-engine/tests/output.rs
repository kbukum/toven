//! Integration coverage for the per-unit raw-output channel under simulated
//! parallel execution: buffered normal units flush a labeled block per unit (in
//! `block` call order), while persistent units live-tail in arrival order, even
//! when their chunks interleave.

use toven_engine::output::{OutputMode, UnitOutputChannel};
use toven_model::{OutputStream, UnitOutput};
use toven_testkit::RecordingRawOutputSink;

fn chunk(unit: &str, bytes: &[u8]) -> UnitOutput {
    UnitOutput {
        unit_id: unit.into(),
        stream: OutputStream::Stdout,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn interleaved_parallel_units_group_per_unit() {
    let sink = RecordingRawOutputSink::new();
    let mut channel = UnitOutputChannel::new(sink.clone());
    channel.register("a", OutputMode::Buffered);
    channel.register("srv", OutputMode::Live);
    channel.register("b", OutputMode::Buffered);

    // Chunks arrive interleaved across three concurrently running units.
    channel.push(chunk("a", b"a1")).unwrap();
    channel.push(chunk("srv", b"s1")).unwrap();
    channel.push(chunk("b", b"b1")).unwrap();
    channel.push(chunk("a", b"a2")).unwrap();
    channel.push(chunk("srv", b"s2")).unwrap();
    channel.push(chunk("b", b"b2")).unwrap();

    // Finish order drives block order; "b" completes before "a".
    channel.finish("b").unwrap();
    channel.finish("a").unwrap();

    // Persistent unit streamed live, in arrival order.
    assert_eq!(
        sink.live_chunks(),
        vec![chunk("srv", b"s1"), chunk("srv", b"s2")]
    );

    // Normal units flushed as one labeled block each, grouped + finish-ordered.
    assert_eq!(
        sink.blocks(),
        vec![
            ("b".to_string(), vec![chunk("b", b"b1"), chunk("b", b"b2")]),
            ("a".to_string(), vec![chunk("a", b"a1"), chunk("a", b"a2")]),
        ]
    );
}
