# Shell completions

Toven generates a shell completion script for a target shell and prints it to stdout. Nothing is written to disk or to your shell configuration; you redirect or source the output yourself.

## Print a completion script

```bash
toven completions <shell>
```

Supported shells: `bash`, `zsh`, `fish`, `powershell`, and `elvish`.

The script is written to stdout so it can be piped or redirected. Diagnostics, if any, stay on stderr.

## Install for your shell

```bash
# zsh: write the script to a directory on your $fpath
toven completions zsh > _toven

# bash: load completions into the current shell
source <(toven completions bash)

# fish
toven completions fish > ~/.config/fish/completions/toven.fish
```

Reload your shell (or re-source the file) after installing.

## Scope

Completions reflect the reserved command surface (`init`, `run`, `plan`, `release`, and the rest) and their flags. Repository-defined task names are resolved from `toven.toml` at run time and are not baked into the generated script; type them directly.
