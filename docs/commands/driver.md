# Manage drivers

Toven runs each ecosystem through a driver. A driver linked into the running binary is used in-process; otherwise Toven can use an out-of-process driver found on `PATH` or pinned in `[toven.drivers]`. A normal run never installs anything — an absent driver is a warn-and-skip — so provisioning is an explicit, opt-in surface behind these verbs.

```bash
toven driver list
```

`driver list` writes its status lines to stdout so they can be piped. Provisioning progress from `driver install` and `driver list --auto-install` uses stderr.

## List driver status

```bash
toven driver list
```

Each canonical ecosystem is reported with its resolved state:

```text
driver: rust -> linked (in this binary)
driver: go -> absent (run `toven driver install <id>`)
```

States: `linked (in this binary)`, `driver on PATH <path>`, `pinned driver <path>`, `pinned driver unavailable …`, and `absent`.

## Install a driver

```bash
toven driver install <id>
```

Installs the out-of-process driver for ecosystem `<id>` (for example `go`). When `[toven.drivers]` pins a version for that ecosystem, the pinned version is installed. An invalid ecosystem id is a usage error; a failed install is surfaced as a typed error.

## Auto-install referenced drivers

```bash
toven driver list --auto-install
```

`--auto-install` provisions every **referenced** ecosystem (declared `[ecosystems.*]` sections and `[toven.drivers]` pins) currently resolved as absent, then reports status. It never provisions drivers for canonical ecosystems the project does not use.

## Pin driver versions

```toml
[toven.drivers]
go = { version = "0.4.1" }
```

A pin makes `driver install`, `federation sync`, and `--auto-install` install that exact version, keeping provisioning reproducible.

See also [federation](federation.md) for provisioning across composed member repositories.
