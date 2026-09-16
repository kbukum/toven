# Completions

Print a shell completion script to stdout for a supported shell.

## Quickstart

Generate a script, then install it the way your shell expects:

```bash
toven completions zsh > _toven
```

Toven writes the script to stdout. It does not write to disk or edit your shell configuration.

## Syntax

```bash
toven completions <shell>
```

## Supported shells

| Shell |
|---|
| `bash` |
| `elvish` |
| `fish` |
| `powershell` |
| `zsh` |

Diagnostics, if any, stay on stderr.

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

## What it covers

The generated script covers Toven's built-in CLI surface. Use `toven --help` to see the current reserved command list.
