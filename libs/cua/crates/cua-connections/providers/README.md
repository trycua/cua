# Provider files

One TOML file per provider. `cargo run -p cua-connections --bin cua-connections-gen`
turns them into the Rust tables, the parity data for the web and SwiftUI apps, the
CLI help and the provider table in `docs/content/docs/spaces/guides/connections.mdx`;
`--check` (and `cargo test -p cua-connections`) fail when any of them is stale.
`schema.rs` is the format and its checks.

## Hiding a provider

`hidden = true` with a `hidden_reason` keeps a provider out of every UI and out of
what apps receive. `terms = "review"` on a provider or a method does the same for a
subscription login whose terms have not been checked. Nothing else is needed to
ship one later: remove the flag.

## Keeping up with Hermes

Provider ids, environment variables and base URLs are checked against a snapshot of
the Hermes release Cua installs (`hermes-pin.json`, `tests/hermes_drift.rs`). To move
the pin:

1. Change the Hermes `commit` and `version` in `crates/cua-agents/installables.json`.
2. Check out that commit and run
   `python3 libs/cua/crates/cua-connections/tools/hermes_snapshot.py <checkout> > libs/cua/crates/cua-connections/hermes-pin.json`.
3. Run `cargo test -p cua-connections`. A provider file that names a variable or URL
   Hermes dropped fails, and so does a new Hermes provider that is neither listed in
   a file nor in `hermes-unlisted.txt` (give it a file, or add it there with the
   reason).
