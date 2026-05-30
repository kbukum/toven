# Changelog

All notable changes to Toven will be documented here by release automation.

## Unreleased

- Added git-baseline affected-module planning with reverse-dependent closure,
  root-file fail-closed behavior, and `toven affected`/`plan --affected` CLI
  surfaces.
- Added strict `toven.toml` loading, normalized workspace/profile/task config,
  and filesystem preset resolution backed by rskit config, validation, and
  filesystem utilities.
