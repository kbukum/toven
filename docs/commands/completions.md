# Shell completions

Print a completion script for your shell:

```bash
toven completions zsh > _toven
```

Toven writes the script to stdout. It does not write to disk or edit your shell configuration.

## Print a script

```bash
toven completions <shell>
```

Supported shells are `bash`, `zsh`, `fish`, `powershell`, and `elvish`. Diagnostics, if any, stay on stderr.

## Install examples

```bash
# zsh: write the script to a directory on your $fpath
toven completions zsh > _toven

# bash: load completions into the current shell
source <(toven completions bash)

# fish
toven completions fish > ~/.config/fish/completions/toven.fish
```

Reload your shell, or re-source the file, after installing.

## Scope

Completions cover reserved commands such as `init`, `run`, `plan`, and `release`, plus their flags. Repository-defined task names come from `toven.toml` at run time, so they are not baked into the generated script. Type them directly.
