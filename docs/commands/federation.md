# Manage federation

Check composed member repositories:

```bash
toven federation status
```

Federation composes several member repositories under one umbrella `toven.toml`. These verbs report and provision the drivers that composed members need. They are the cross-repo counterpart to [driver management](driver.md).

## Report federation status

```bash
toven federation status
```

`federation status` writes member and driver status lines to stdout so they can be piped.

```text
federation: rust -> linked (in this binary)
federation: go -> driver on PATH /usr/local/bin/toven-go
```

The command is read-only and installs nothing.

## Synchronize pinned drivers

```bash
toven federation sync
```

`federation sync` installs every version-pinned driver in `[toven.drivers]`. That lets a fresh checkout of the umbrella repo provision the same driver set. Provisioning progress uses stderr.

Path-pinned drivers are configuration errors for `sync` because Toven cannot install a local binary path. With no pins, Toven reports:

```text
federation sync: no pinned drivers in [toven.drivers]; nothing to install
```

Add `--auto-install` to also provision referenced-but-absent ecosystem drivers after syncing pins. The first install failure stops the sync with a typed error.

## Pin the driver set

```toml
[toven.drivers]
go = { version = "0.4.1" }
```

Pins make `federation sync` reproducible across member checkouts. See [driver management](driver.md) for the single-repo view and the full list of resolved driver states.
