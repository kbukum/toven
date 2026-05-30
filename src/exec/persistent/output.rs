use crate::exec::{PersistentOutput, PersistentOutputStream};

pub(super) const fn map_output(output: PersistentOutput) -> rskit_process::PersistentOutput {
    match (output.stdout_stream(), output.stderr_stream()) {
        (Some(stdout), Some(stderr)) => {
            rskit_process::PersistentOutput::forward(map_stream(stdout), map_stream(stderr))
        }
        _ => rskit_process::PersistentOutput::capture_only(),
    }
}

const fn map_stream(stream: PersistentOutputStream) -> rskit_process::PersistentOutputStream {
    match stream {
        PersistentOutputStream::Stdout => rskit_process::PersistentOutputStream::Stdout,
        PersistentOutputStream::Stderr => rskit_process::PersistentOutputStream::Stderr,
    }
}
