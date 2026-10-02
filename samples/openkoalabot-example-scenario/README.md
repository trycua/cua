# openkoalabots scenario

One language-neutral scenario, `scenario.json`, that every openkoalabots
implementation runs headlessly. The three implementations share it:
[`openkoalabot-example-swift`](../openkoalabot-example-swift), [`openkoalabot-example-tauri`](../openkoalabot-example-tauri)
and [`openkoalabot-example-ts`](../openkoalabot-example-ts). Each one runs the spec through its
own app core, so a passing run shows that the app's code works against a
real Space. The spec only fixes what must be true. Each language decides how
its runner gets there.

| Step | Operation | What must be true |
|---|---|---|
| `space` | add (`fixture`, `docker`) or create (`cloud`) a Space | the Space is registered in a **temp** registry after a capabilities handshake |
| `stream` | desktop stream session | at least 1 frame arrives, the first frame is a keyframe, and a keyframe request produces another. Skipped when the Space lacks `desktop_stream` |
| `agent` | agent thread with a **fake** `claude` CLI | turn 1 (`agent_start`) and turn 2 (`agent_message`, which resumes with `--continue`) both reach `idle`, and each output tail contains the expected line |
| `file` | `send_file` of a generated 1 MiB file | the bytes come from the spec's xorshift64 generator, the local sha256 equals `sha256`, and the guest's own `sha256sum` agrees |
| `teleport` | teleport `firefox` with an explicit approval callback | the profile is **generated** from `fixtures/firefox-profile.json` in a temp teleport home, and the run's marker is found in the guest |
| `presence` | two independent SDK runtimes join | the operator sees Koala join, then sees its cursor move to (0.25, 0.75), then sees it leave; the SDK's presence roster, folded from those events, shows Koala's cursor and then drops Koala. The operator holds Koala's stable color, so the server assigns Koala another; the app's avatar color for Koala must equal that cursor color. Skipped without `presence` |
| `routine` | the app's routine store and live runner | a routine not yet due does not fire; the real scheduler fires it once, as a real agent turn whose prompt starts `[routine] <title>:` and whose output has the scripted reply; another tick fires nothing; a reloaded store has the run id. Skipped without a model endpoint |
| `group` | the app's group store and live messenger, two Bots | groups of 1 and 7 are refused; one message reaches both Bots, framed with the other's name; each Bot's reply is attributed to it once. Skipped without a model endpoint |
| `delete` | delete the Space | always runs. A Space added by address is only removed from the registry; a created cloud Space is deleted |

No step calls a real model or uses real credentials: agent turns run the real harness against `cua-mock-llm`, whose `mock:` directive in the prompt scripts the reply. No step reads `~/.cua`,
`~/.claude`, `~/.codex`, a real browser profile or the keychain. No step
opens a window.

## Runner contract

```
<runner> --spec scenario.json --lane fixture|docker|cloud --out result.json
```

| Variable | Meaning |
|---|---|
| `OPENKOALABOTS_SCENARIO_URL`, `OPENKOALABOTS_SCENARIO_TOKEN` | the spacesd to add (`fixture`, `docker`) |
| `OPENKOALABOTS_SCENARIO_IMPORT_ROOT` | `{importRoot}` in `teleport.verifyCommand`: the literal `$HOME` in a real guest, or the fixture's teleport import directory |
| `OPENKOALABOTS_SCENARIO_MODEL_URL` | the model endpoint as the Space reaches it (`cua-mock-llm`); the runner forwards `ANTHROPIC_API_KEY` from its own environment. Unset: `routine` and `group` skip |
| `OPENKOALABOTS_CLOUD_IMAGE` | `cloud` lane: the linux (spacesd) image to create in Cua Cloud. Credentials come from the usual environment (`cua auth login`) |

The result is `{impl, lane, ok, totalMs, steps: [{id, status, ms, detail}]}`,
where `status` is `pass`, `fail` or `skip`. A run is `ok` when no step failed.

| Implementation | Runner |
|---|---|
| swift | `samples/openkoalabot-example-swift/.build/debug/OpenKoalaBotExample scenario …` |
| tauri | `samples/openkoalabot-example-tauri/src-tauri/target/debug/openkoalabot-example-scenario …` |
| ts | `node samples/openkoalabot-example-ts/dist/scenario/cli.js …` |

## Lanes

```sh
# Hermetic: an in-process spacesd server core (libs/cua cua-test-fixtures), confined to a temp HOME and PATH.
samples/openkoalabot-example-scenario/run.sh --impl all --lane fixture

# A local linux image container under gVisor, --memory=4g, one at a time.
samples/openkoalabot-example-scenario/run.sh --impl all --lane docker \
    [--image cua-e2e-local/linux:docker-local-arm64] [--runtime runsc]

# Cua Cloud (gated): needs a linux (spacesd) image the cloud can pull.
OPENKOALABOTS_CLOUD_IMAGE=ghcr.io/trycua/linux:24.04 \
    samples/openkoalabot-example-scenario/run.sh --impl all --lane cloud
```

`run.sh` writes `results/<impl>-<lane>.json` and prints an
implementation × step matrix. Build the runners first; each implementation's
README has the command.

The `docker` lane also starts `cua-mock-llm` (`--memory=256m`, removed with the Space) and sets the model variables, so `routine` and `group` run real agent turns; the first turn installs the harness in the Space (about 30 s). Point `--mock-llm` at a Linux build of `cua-mock-llm` (`cargo zigbuild --release -p cua-mock-llm --target aarch64-unknown-linux-musl` in libs/cua).

The `fixture` lane has no desktop, so `stream` reports `skip`, and no model, so `routine` and `group` skip. The spacesd
server core runs on the host with a temp `HOME`, and `presence` and
`teleport` run against it for real.
