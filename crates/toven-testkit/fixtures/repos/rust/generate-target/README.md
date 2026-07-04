# generate-target

A rust workspace deliberately shipped **without** a `toven.toml`, so the `generate` smokes can render (`--stdout`) and scaffold (`--write`) a fresh config and then observe the idempotent "already exists" path on a second write.
