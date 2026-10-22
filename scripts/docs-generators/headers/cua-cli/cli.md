## Output, errors and verbosity

stdout carries results and data. stderr carries progress, logs, warnings, errors and notices, so `cua ... | jq` and `cua ... > file` stay clean.

An error is two lines on stderr: what went wrong in one sentence, then what to do about it.

```text output
error: a sandbox named 'dev' already exists
fix: pick another name, or remove it with `cua sb rm dev`
```

With `--json`, the same error is one object on stdout, with the same exit code:

```json
{"error": {"code": "already_exists", "message": "a sandbox named 'dev' already exists", "hint": "pick another name, or remove it with `cua sb rm dev`"}}
```

The internal layers an error passed through, and the raw text, appear as a `detail:` line (a `detail` field in JSON) with `-v`.

| Flag | Shows |
| --- | --- |
| `-q`, `--quiet` | Errors only. |
| (default) | Errors, warnings, and the steps of a create on a terminal. |
| `-v` | What Cua chose and did. |
| `-vv` | Debug detail. |
| `-vvv` | Everything, dependencies included. |

`--log-level error|warn|info|debug|trace` and the `CUA_LOG_LEVEL` variable say the same in words; `-q` and `-v` win over them. `CUA_LOG` takes a [`tracing` filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives) (`CUA_LOG=cua_vmm=debug`) for experts and wins over every other setting.

`--color auto|always|never` picks colour. `auto` colours a terminal, unless `NO_COLOR`, `TERM=dumb` or `CI` is set. In those cases, and when stderr is not a terminal, steps are printed once as plain lines and nothing rewrites a line in place.

`-V` and `--version` print the version. `-v` used to; a lone `cua -v` still prints the version, with a notice, for now.

### Error codes

| `code` | Exit | Meaning |
| --- | --- | --- |
| `invalid_argument` | `2` | A flag or argument has a value Cua cannot use. |
| `already_exists` | `2` | A sandbox with that name exists; pick another name or delete it. |
| `ambiguous_sandbox` | `2` | A bare sandbox name matches more than one location; use `local:NAME`. |
| `invalid_placement` | `2` | An `--on`, `--kind` or `--runtime` that does not exist or does not fit the image. |
| `not_found` | `3` | No such sandbox, process, path, forward or session. |
| `image_not_found` | `3` | No image by that name or reference. |
| `image_not_published` | `3` | A catalog image CI has not published yet. |
| `unsupported` | `4` | This provider, image, runtime or build cannot do that. |
| `provider_not_configured` | `4` | A provider is missing its credentials or helper. |
| `host_capability_missing` | `1` | This computer lacks something the command needs. |
| `capability_missing` | `1` | The Space's cua-spacesd lacks a feature the command needs. |
| `spacesd_not_available` | `5` | No cua-spacesd answered in the sandbox. |
| `transport` | `5` | Could not reach the daemon or the endpoint. |
| `daemon_not_running` | `5` | No `cua daemon` is running. |
| `network` | `5` | The network refused or could not be reached. |
| `unauthenticated` | `6` | Not signed in, or a missing or wrong token. |
| `permission_denied` | `6` | The guest or the service denied permission. |
| `registry_auth` | `6` | An image registry refused the pull. |
| `cloud_credentials` | `6` | Your cloud account has no credentials or rejected them. |
| `cloud` | `6` | Your own cloud account refused or failed a call. |
| `insufficient_disk` | `7` | Not enough free disk space. |
| `cancelled` | `130` | The command was cancelled (Ctrl-C); what it made was removed. |
| `timeout` | `1` | A deadline elapsed. |
| `runtime` | `1` | A local runtime failed. |
| `env` | `1` | A cua-spacesd call failed. |
| `http` | `1` | An HTTP call to a service failed. |
| `closed` | `1` | The object was closed. |
| `teleport_refused` | `1` | The teleport consent gate refused. |
| `internal` | `1` | A bug or an I/O failure. |
