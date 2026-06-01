use std::path::Path;

pub(super) fn command_from_argv(
    argv: &[String],
    workspace_root: &Path,
) -> Result<rskit_process::ProcessSpec, ()> {
    let Some((program, arguments)) = argv.split_first() else {
        return Err(());
    };
    Ok(rskit_process::ProcessSpec::new(program)
        .args(arguments.iter().map(std::ffi::OsString::from))
        .dir(workspace_root))
}
