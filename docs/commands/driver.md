# Manage drivers

List the drivers Toven can use:

```bash
toven driver list
```

Toven runs each ecosystem through a driver. A driver linked into the running binary runs in-process. Otherwise Toven can use an out-of-process driver from `PATH` or one pinned in `[toven.drivers]`.

A normal run never installs anything. An absent driver is a warn-and-skip, so provisioning is explicit through these verbs.

## List driver status

```bash
toven driver list
```

`driver list` writes status lines to stdout. Provisioning progress from `driver install` and `driver list --auto-install` uses stderr.

```text
driver: rust -> linked (in this binary)
driver: go -> absent (run `toven driver install <id>`)
```

States are `linked (in this binary)`, `driver on PATH <path>`, `pinned driver <path>`, `pinned driver unavailable …`, and `absent`.

## Install a driver

```bash
toven driver install <id>
```

This installs the out-of-process driver for ecosystem `<id>`, such as `go`. When `[toven.drivers]` pins a version for that ecosystem, Toven installs that version. An invalid ecosystem id is a usage error, and a failed install surfaces as a typed error.

## Auto-install referenced drivers

```bash
toven driver list --auto-install
```

`--auto-install` provisions every referenced ecosystem driver that is absent. Referenced ecosystems come from declared `[ecosystems.*]` sections and `[toven.drivers]` pins. Toven never provisions drivers for canonical ecosystems the project does not use.

## Pin driver versions

```toml
[toven.drivers]
go = { version = "0.4.1" }
```

A pin makes `driver install`, `federation sync`, and `--auto-install` install that exact version. Use pins for reproducible provisioning.

See [federation](federation.md) for provisioning across composed member repositories.
