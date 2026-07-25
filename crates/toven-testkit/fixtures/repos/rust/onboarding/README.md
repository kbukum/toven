# onboarding

A rust workspace deliberately shipped **without** a `toven.toml`, so the `init` smokes can render (`--print`) and write (default) a fresh config and then observe the idempotent "already exists" path on a second run.
