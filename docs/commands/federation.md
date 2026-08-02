# Manage federation

Federation composes several member repositories under one umbrella `toven.toml`. These verbs report and provision the drivers that the composed members need. They are the cross-repo counterpart to [driver management](driver.md).

```bash
toven federation status
```

`federation status` writes member and driver status lines to stdout so they can be piped. Provisioning progress from `federation sync` uses stderr.

## Report federation status

```bash
toven federation status
```

Reports the resolved provisioning state of every canonical ecosystem for the composed project, one line per ecosystem:

```text
federation: rust -> linked (in this binary)
federation: go -> driver on PATH /usr/local/bin/toven-go
```

This is read-only — it installs nothing.

## Synchronize pinned drivers

```bash
toven federation sync
```

Installs every version-pinned driver in `[toven.drivers]`, so a fresh checkout of the umbrella provisions the same driver set. Path-pinned drivers are reported as configuration errors for `sync` because Toven cannot install a local binary path. With no pins it reports that there is nothing to install:

```text
federation sync: no pinned drivers in [toven.drivers]; nothing to install
```

Add `--auto-install` to also provision any referenced-but-absent ecosystem drivers after syncing the pins. The first install failure stops the sync with a typed error.

## Pin the driver set

```toml
[toven.drivers]
go = { version = "0.4.1" }
```

Pins make `federation sync` reproducible across every member checkout. See [driver management](driver.md) for the single-repo view and the full list of resolved driver states.
