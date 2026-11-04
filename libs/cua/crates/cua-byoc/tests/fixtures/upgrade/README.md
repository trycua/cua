# Cua homes written by released cua builds

`tests/upgrade.rs` seeds a cua home from each `*/home` here and checks that
this cua reads and manages it with no step by the person. Every id, account
and token in them is fake: they were written against fake clouds (the fake
EC2/STS and Hetzner servers in `tests/support/`), in a throwaway `HOME`, with
`CUA_CREDENTIAL_STORE=file` (never a keychain) and telemetry off.

## v0.7.0

Written by the released `cua` 0.7.0 binary (`cua-cli-0.7.0-darwin-arm64.tar.gz`
from the `cua-sdk-v0.7.0` release), with the fakes served by
`cargo test -p cua-byoc --features all --test upgrade -- --ignored serve_fakes_for_a_capture`
(`CUA_UPGRADE_SERVE_FILE=<file>` gets their URLs):

    capture.sh <cua 0.7.0> <out> $AWS_ENDPOINT_URL cloud connect aws --profile upgrade-fake --region us-west-2 --ttl-hours 4 --default --embedded

`home/` is `<out>/.cua`: `cloud/state.json` and `config.toml`. cua 0.7.0
records a Space's machines only after a cua.ai sign-in, so this home has no
records; its record format is the same as 0.8.0's (`Resource` and `State`
did not change between the two tags).

## v0.8.0

The last release before federation (`cua-sdk-v0.8.0`, before #247). The
`cua` binary built from that tag (`cargo build -p cua-cli --features byoc`)
ran:

    capture.sh <cua 0.8.0> <out> $AWS_ENDPOINT_URL cloud connect aws --profile upgrade-fake --region us-west-2 --ttl-hours 4 --embedded
    capture.sh <cua 0.8.0> <out> $AWS_ENDPOINT_URL cloud budget --max-per-hour 2.5 --monthly 300 --autoscale off --max-new-hosts 3 --embedded

Its Hetzner API root is fixed, so the Hetzner link (token on standard input,
saved to the Cua credential store), a pool host there, and an AWS Space's
machine were made by that tag's own `cua-byoc` code against the fakes and a
fake relay: `capture_v0_8_0.rs.txt` (dropped into that tag's
`crates/cua-byoc/tests/` and run with `CUA_UPGRADE_CAPTURE_HOME=<out>/.cua`).

`home/` is `<out>/.cua`: `cloud/state.json`, `cloud/audit.jsonl`,
`elastic/policy.json` and `byoc-hetzner-token` (the credential store file).

## Running a released cua against a home this cua wrote

    CUA_UPGRADE_OLD_CUA=/path/to/cua-0.7.0 cargo test -p cua-byoc --features all --test upgrade released_cua

runs that binary on a home with a federated link (it must refuse it, not use
the default AWS profile) and lets it write the file back (this cua then puts
the federated link and its limits back from `cloud/links.json`).
