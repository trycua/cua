# cua-e2b

An `e2b`-compatible Python module whose sandboxes are Cua Spaces: Linux VMs
on your own machines. Code written for the e2b SDK (2.42, 2.56 and 2.58) keeps working after
you swap the dependency:

```diff
-dependencies = ["e2b==2.42.0"]
+dependencies = ["cua-e2b"]
```

```python
from e2b import AsyncSandbox, CommandExitException

sandbox = await AsyncSandbox.create(template="my-world", timeout=3600)
try:
    r = await sandbox.commands.run("uname -a", user="root")
    await sandbox.files.write("/tmp/in.tar", data, user="root")
    handle = await sandbox.commands.run("./tool", background=True, timeout=0, on_stdout=print)
    await handle.wait()
    mcp_url = f"http://{sandbox.get_host(8765)}/mcp"
except CommandExitException as e:
    print(e.exit_code, e.stderr)
finally:
    await sandbox.kill()
```

`cua-e2b` installs a top-level `e2b` module, so it replaces the `e2b`
package: install one or the other in an environment.

## How e2b maps to Cua

| e2b | Cua |
|---|---|
| `create(template=...)` | `template` names a Cua checkpoint: the sandbox is **forked** from it (memory and disk). Otherwise it is an image, and a new Space of kind `vm` boots from it. `None` / `"base"`: the canonical Linux image; `"macos"`, `"windows"`, `"linux"`: Cua's image of that OS. |
| placement | `on="auto"`: the least-loaded of your machines (`CUA_E2B_ON`). |
| `timeout`, `set_timeout`, `lifecycle` | a Space lifetime kept by the cua daemon: it deletes the Space (or suspends it, `on_timeout="pause"`) even after your process exits. |
| `allow_internet_access=False`, `network={"deny_out": [ALL_TRAFFIC]}` | the VM gets no network (`network="none"`); the Cua channel and `get_host` still reach it. |
| `sandbox_id` | the Space's name |
| `commands.run` (foreground, background, `on_stdout`/`on_stderr`, `timeout`, `user`, `envs`, `cwd`, `stdin`), `list`, `kill`, `send_stdin`, `close_stdin`, `connect` | cua-spacesd processes, run as `/bin/bash -l -c <cmd>` like e2b; `user="root"` (or any other user) runs through `sudo -n` (natively, with no `sudo`, on a cua-spacesd that advertises `process.run_as`), and kill stops the whole process group |
| `pty.create/send_stdin/resize/kill/connect` | a cua-spacesd PTY running `/bin/bash -i -l` |
| `files.read/write/write_files/remove/list/exists/get_info/make_dir/rename/watch_dir` | cua-spacesd's file service (SHA-256 checked); files written for a user belong to that user; a refused call is repeated as root and given to the user; metadata is kept as `user.e2b.*` xattrs. A cua-spacesd that advertises `fs.run_as` / `fs.xattr` does all of this with its own file calls (read once per sandbox); an older image keeps the equivalent `sudo` / `python3` helper commands |
| `files.read(format="stream")` | one resumable streamed read, a few chunks buffered (memory stays bounded) |
| `download_url(path)`, `upload_url(path)` | Signed `/files` URLs minted by cua-spacesd (GET download; PUT or multipart POST upload; 24 hours at most; for another `user` they need a cua-spacesd with `fs.run_as`, which an older image lacks). |
| `create(network=...)`, `update_network(...)` | `allow_out`/`deny_out` with IP addresses and CIDR blocks are rules the core enforces inside the Space (allow entries win over deny); hostnames, `rules` and `egress_proxy` raise `InvalidArgumentException` with the way out. Needs a Linux image with nftables. |
| `create(mcp=...)`, `get_mcp_url`, `get_mcp_token` | Starts `mcp-gateway` as root with a fresh token, as e2b does; the template must provide the `mcp-gateway` program. |
| `get_host(port)` | a TCP forward through cua-spacesd's tunnel, `127.0.0.1:<port>` (plain TCP: use `http://`); works for a Space on any of your machines |
| `kill()` | deletes the Space |
| `create_snapshot(name, mode=)`, `list_snapshots()`, `delete_snapshot(id)` | a pinned Cua checkpoint (copy-on-write where the runtime can); `create(template=name)` forks it. `mode="filesystem"`: sandboxes made from it cold-boot from the disk |
| `fork(count=, timeout=)` | `count` running copies of a live sandbox (disk and memory), the parent keeps running; a fork that fails is its exception in the list |
| `pause()` / `connect()` / `lifecycle={"auto_resume": True}` | suspend / resume, memory kept |
| `pause(mode="filesystem")` / `keep_memory=False`, `connect(on_resume="reboot")`, `lifecycle={"on_timeout": {"action": "pause", "mode": "filesystem"}}` | the resume cold-boots the guest from its disk: files stay, processes and memory do not |
| `list(order="asc"/"desc")` | by start time |
| `list()`, `get_info()`, `get_metrics()` | your `e2b-*` Spaces; info and a CPU/memory/disk sample read in the guest |
| exceptions | the same classes; a non-zero exit raises `CommandExitException` with e2b's `error` text |

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CUA_E2B_ON` | `auto` | Where new sandboxes go: `auto` (the least-loaded of your signed-in machines), `local`, `host:<machine>`, `cloud` (Cua Cloud), your own cloud (`aws`, `gcp`, `modal`, ... after `cua cloud add`), or `direct:<url>` (an existing machine running cua-spacesd, with `CUA_E2B_TOKEN`) |
| `CUA_E2B_KIND` | `vm` | Space kind: `vm`, `container` (gVisor), `microvm` |
| `CUA_E2B_IMAGE` | canonical Linux | Image for `template=None` / `"base"` |
| `CUA_E2B_CPUS`, `CUA_E2B_MEMORY_MB`, `CUA_E2B_DISK_GB` | Space defaults | VM size |
| `CUA_E2B_TEMPLATES` | (none) | JSON object, or a path to one, mapping e2b template/snapshot ids to checkpoint names or images |
| `CUA_E2B_MODE` | `auto` | `daemon`, `embedded`, or `auto` (the cua daemon, started with the bundled CLI when none runs) |
| `CUA_E2B_DAEMON` | (none) | daemon address |
| `CUA_E2B_BIND` | `127.0.0.1:0` | where `get_host` forwards listen (`0.0.0.0:0` serves the LAN) |

## Platform features: what maps, and what to do instead

| e2b feature | Cua | Notes |
|---|---|---|
| `Template().from_...().run_cmd(...)...` (every builder method, `from_dockerfile`, `copy` with `.dockerignore`), `Template.build(template, name)`, `build_in_background`, `get_build_status`, `exists`, `to_dockerfile`, `to_json` | the recipe runs in a Linux VM Space, which is saved as a pinned snapshot (memory included); `create(template=name)` forks it | Start from `from_base_image()`, `from_ubuntu_image()`, `from_image("linux")`, one of Cua's images, or `from_template(name)`. The started service is already running in every sandbox made from it. There is no layer cache, so every build runs all steps (`skip_cache` and `force_upload` hold by construction). `cpu_count` / `memory_mb` size the builder VM and so every sandbox forked from the template; `min_free_disk_mb` is checked after the steps (a VM disk is sized at creation: `CUA_E2B_DISK_GB`). `get_build_status` follows builds of the same process. |
| template tags: `name:tag`, `build(tags=[...])`, `assign_tags`, `remove_tags`, `get_tags` | one record per tag next to the Spaces registry, each pointing at the snapshot of one build; `create(template="name:tag")` forks it | Every build also moves the `default` tag, which is what a plain `name` means. A snapshot is deleted when no tag points at its build any more. |
| template bases that are bare OCI images (`from_python_image`, `from_image("node:22")`, a Dockerfile's other `FROM`) and registry credentials (`from_image(username=, password=)`, `from_aws_registry`, `from_gcp_registry`) | refused when the build starts | Cua Spaces boot Cua images (VM disks that carry cua-spacesd), not OCI root filesystems, so there is nothing to run the steps in. Start from `from_base_image()` and use `apt_install` / `pip_install` / `run_cmd`, or build a Cua image with `cua images build` and pass it to `from_image(...)`. |
| `Volume.create/connect/list/destroy/get_info`, instance `list(path, depth)` / `get_info(path)`, `read_file` (text, bytes, stream), `write_file`, `make_dir`, `exists`, `remove`, `update_metadata` | the folder `e2b-volumes/<name>/` of Cua Volume (one versioned volume per user: Finder, every Space, your own storage) | Needs the daemon Cua Spaces runs (Cua Volume ships with Cua Spaces); a plain `cua daemon` answers with a clear error. A volume is an object store: a directory exists while it holds files (`make_dir` keeps a marker), and `uid`/`gid`/`mode` are kept as metadata, returned by the stat calls and not enforced. `write_file` and `make_dir` need the parent directory unless `force=True` creates it. A volume has no token or content host: `token` is empty and `domain`/`proxy`/`http_version` are kept on the object and unused. Every write is a new version. |
| `create(volume_mounts={"/data": volume})` | a symlink from the path to `/volume/e2b-volumes/<name>` where the Space mounts Cua Volume | A Space without the mount (QEMU VMs do not mount it yet) refuses with that reason; use `Volume.read_file/write_file` from your code, or `files.write` on the sandbox. |
| `create(iam=...)` | none | The option asks e2b's control plane for short-lived cloud credentials inside the sandbox. Cua has no such broker: keep the credentials in Keyvault or pass them with `envs=` (`cua run --env KEY`), or run the Space in your own cloud account (`CUA_E2B_ON=aws`) so its machine role applies. |

## Differences from e2b

- `get_host` returns a loopback `host:port`: use `http://`, not `https://`.
- `secure`, `allow_internet_access` and `pause(keep_memory)` default to
  `None` as in e2b 2.56 (True in 2.42): both mean the same here.

## Tests

```bash
cd libs/python/cua-e2b
uv run --with pytest --with pytest-asyncio --with-editable . python -m pytest tests
```

- `tests/test_e2b_semantics.py`: e2b semantics over an in-memory backend.
- `tests/test_conformance.py`: the API golden and the behaviour golden,
  recorded from e2b 2.42.0 and 2.56.0 / E2B (`tests/conformance/golden/`); the shim meets both. The
  behaviour case runs the conformance suite through a real cua-spacesd
  (the binding fixture) in the `CI: cua SDK` workflow.
- `tests/conformance/suite.py`: the suite itself, run against E2B and Cua
  alike. `.github/workflows/e2b-conformance.yml` re-runs it weekly against
  the latest e2b release (and E2B, with `E2B_API_KEY`), and against Linux
  VM Spaces on a VM-capable runner.
- `tests/smoke_templates.py`: a volume round trip, a VM template built from a recipe and forked, and the volume_mounts answer, on real Spaces.
- `tests/smoke_live.py`: an RL-rollout-shaped run on real Spaces (bake,
  checkpoint, fork, tool calls, files, `get_host`).
