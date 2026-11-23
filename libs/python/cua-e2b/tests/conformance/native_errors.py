"""Calls that exist in cua-e2b but cannot do what e2b's do in some case: each
raises e2b's own exception class with a message that names the way out, or
(`difference`) behaves differently in a documented way. Rendered into
`COVERAGE.md`; `tests/test_coverage_matrix.py` checks the test references.

`(api, case, outcome, reason, test)`; outcome is `native-error: <ExceptionClass>`
or `difference`.
"""

LIMITS = [
    (
        "Sandbox.create(iam=...)",
        "a non-empty `tokens` map",
        "native-error: InvalidArgumentException",
        "E2B's control plane mints short-lived workload identity tokens that an egress proxy injects; Cua Spaces have no credential broker. Pass credentials with `envs=` (`cua run --env KEY`), keep them in Keyvault, or run the Space in your own cloud account so its machine role applies.",
        "tests/test_e2b_semantics.py::test_mcp_starts_the_gateway_as_e2b_does_and_iam_is_refused",
    ),
    (
        "Sandbox.create(network=...), Sandbox.update_network(...)",
        "`rules` (header transforms) or `egress_proxy`",
        "native-error: InvalidArgumentException",
        "They are applied by a TLS-intercepting proxy in front of the sandbox, which Spaces do not have. `allow_out`/`deny_out` with IP addresses and CIDR blocks are enforced inside the Space by the core (nftables); credentials the sandbox needs go in `envs=`.",
        "tests/test_e2b_semantics.py::test_proxy_rules_are_refused_not_dropped",
    ),
    (
        "Sandbox.create(network=...), Sandbox.update_network(...)",
        "hostnames in `allow_out`/`deny_out`",
        "native-error: InvalidArgumentException",
        "E2B matches hostnames by SNI in its proxy; the core enforces IP/CIDR rules inside the Space and rejects hostnames instead of resolving them once (addresses rotate).",
        "tests/test_e2b_semantics.py::test_ip_rules_become_egress_rules_in_the_space",
    ),
    (
        "Sandbox.download_url / upload_url",
        "`user` other than the sandbox's default user, on a cua-spacesd without `run_as`",
        "native-error: InvalidArgumentException",
        "A cua-spacesd that advertises `fs.run_as` signs the URL for that user (`CreateSignedUrl` with `run_as`, `ru` in the query); an older one would sign as its own user, which is refused rather than done silently. Rebuild the image, or use `files.read`/`files.write(user=...)`.",
        "tests/test_sandbox_conformance_fixes.py::test_signed_url_for_another_user_needs_spacesd_run_as",
    ),
    (
        "Sandbox.download_url / upload_url",
        "the URL itself, and a call on a sandbox that is gone",
        "difference",
        "The URL is cua-spacesd's (`path`, `method`, `exp`, `sig`; another user as `ru`), not E2B's `signature` / `signature_expiration` / `username`, and the daemon mints it: a killed sandbox raises SandboxNotFoundException where E2B, which signs locally, still returns a URL.",
        "tests/test_sandbox_conformance_fixes.py::test_signed_url_of_a_killed_sandbox_is_not_found",
    ),
    (
        "Sandbox.get_host(port)",
        "a port nothing listens on",
        "difference",
        "E2B's public proxy answers HTTP 502 with a JSON body; the forward is a raw TCP tunnel, so the connection is reset.",
        "tests/test_workload_shape.py::test_tool_server_hosts_and_kill_with_a_request_timeout",
    ),
    (
        "Sandbox.envd_access_token / traffic_access_token",
        "a secure sandbox (the default)",
        "difference",
        "E2B returns the token its REST port wants; cua-spacesd is reached through the Cua channel and has no per-sandbox token to give out, so the attribute is None rather than a made-up value.",
        "tests/test_sandbox_conformance_fixes.py::test_sandbox_has_no_envd_token",
    ),
    (
        "Sandbox.update_network(...) / create(network=...)",
        "a hostname in `allow_out`, or an `allow_out` entry that is not an address",
        "native-error: InvalidArgumentException",
        "E2B treats such an entry as a domain and filters by SNI; the core filters by IP/CIDR. `deny_out` entries and the allow-wins-over-deny precedence are identical, and a malformed `deny_out` entry fails with E2B's `400: invalid denied CIDR <x>`.",
        "tests/test_sandbox_conformance_fixes.py::test_network_updates_are_checked_like_e2bs_api",
    ),
    (
        "Sandbox.download_url / upload_url",
        "`use_signature_expiration` over 24 hours (and no expiration)",
        "native-error: InvalidArgumentException",
        "cua-spacesd signs file URLs for at most 24 hours; the default is 24 hours.",
        "tests/test_runtime_fidelity.py::test_download_and_upload_urls_are_signed_by_spacesd",
    ),
    (
        "Secret.create / update / get_info / list / exists / destroy",
        "every call",
        "native-error: SecretException",
        "E2B's account-level secret store feeds its egress proxy and workload identity; Cua has no such store reachable from a Space. `Secret.fill` and `Secret.iam_token`, which only build values, work. Use `envs=` or Keyvault.",
        "tests/test_secret_errors.py",
    ),
    (
        "ApiClient, AsyncApiClient, e2b.api.client",
        "any request",
        "native-error: SandboxException",
        "They are E2B's REST transport; cua-e2b has no REST control plane behind it (sandboxes are Spaces).",
        "tests/test_config_types.py",
    ),
    (
        "Template.from_python_image / from_image('node:22') / registries / Dockerfile FROM",
        "a base that is a bare OCI image",
        "native-error: BuildException",
        "Cua Spaces boot Cua images (VM disks that carry cua-spacesd), not OCI root filesystems; start from `from_base_image()` and install with `apt_install`/`pip_install`/`run_cmd`, or build a Cua image.",
        "tests/test_template_modules.py",
    ),
    (
        "Sandbox.create(volume_mounts=...)",
        "a Space without Cua Volume mounted at /volume (QEMU VMs)",
        "native-error: InvalidArgumentException",
        "The mount is a symlink to the volume's folder of Cua Volume, which the Space must mount; use `Volume.read_file/write_file` or `files.write`.",
        "tests/test_e2b_semantics.py",
    ),
    (
        "Sandbox.get_host(port)",
        "the URL scheme",
        "difference",
        "A loopback `host:port` that forwards raw TCP through cua-spacesd's tunnel, so use `http://` (code that hard-codes `https://` needs the gateway, `cua run --compat e2b`, which serves TLS on those names). Reaches a Space on any of your machines; `CUA_E2B_BIND=0.0.0.0:0` serves the LAN.",
        "tests/test_workload_shape.py::test_tool_server_hosts_and_kill_with_a_request_timeout",
    ),
    (
        "Sandbox.get_metrics(start, end)",
        "a window in the past",
        "difference",
        "cua-spacesd keeps no metric history: the current sample is returned for a window that contains now, an empty list otherwise.",
        "tests/test_runtime_fidelity.py",
    ),
    (
        "Sandbox.create(secure=...)",
        "any value",
        "difference",
        "Deprecated upstream and ignored there too (every sandbox secures envd access); cua-spacesd always requires its token.",
        "tests/test_e2b_semantics.py",
    ),
    (
        "files.write",
        "overwriting a file that belongs to the user in a sticky world-writable directory such as /tmp",
        "difference",
        "e2b's envd overwrites in place as root, which the kernel refuses there (protected_regular: `500: error opening file ... permission denied`). cua-spacesd replaces the file atomically by renaming a temporary file into place, so the rewrite works and a reader never sees half a file.",
        "tests/test_files_fidelity.py::test_a_file_in_a_sticky_directory_can_be_rewritten",
    ),
    (
        "files.get_info / list / write (owner, group, home directory)",
        "the default user's name and home directory",
        "difference",
        "The Space's default user is the image's `cua` with home /home/cua; e2b's template user is `user` with home /home/user. Relative paths resolve against the real home, and `user=\"user\"` means the Space's default user.",
        "tests/test_e2b_semantics.py::test_write_gives_the_file_to_the_user",
    ),
    (
        "files.get_info / list",
        "`size` of a directory",
        "difference",
        "It is the number the guest's file system reports for a directory (an e2b sandbox's root file system reports a different one).",
        "tests/test_files_fidelity.py::test_a_directory_reports_the_size_its_file_system_gives",
    ),
    (
        "files.write / write_files",
        "`metadata` with about 200 keys",
        "difference",
        "e2b's edge answers `431: Request Header Fields Too Large` because metadata travels in HTTP headers; cua-e2b sends it over cua-spacesd's gRPC and enforces envd's own limits (key at most 246 bytes, 4096 bytes in all) with the same texts.",
        "tests/test_files_fidelity.py::test_metadata_limits_are_envds",
    ),
    (
        "files.remove",
        "a path the guest cannot remove (for example below /proc)",
        "difference",
        "The error is `Code.INTERNAL: error removing file or directory: ` followed by the guest's own text (`rm: cannot remove ...`); e2b's follows Go's text (`openfdat ...`).",
        "tests/test_files_fidelity.py::test_a_refused_removal_is_an_internal_error_with_the_guests_words",
    ),
    (
        "files.write / read",
        "`gzip`, `use_octet_stream`, `stream_idle_timeout` with `format != 'stream'`",
        "difference",
        "They choose how e2b encodes its HTTP transfer; the transfer to cua-spacesd has its own chunked, SHA-256 checked framing, so they change nothing (the data is identical).",
        "tests/test_runtime_fidelity.py::test_write_streams_file_objects_and_encodes_text_io",
    ),
    (
        "commands.run / pty.create, files (relative paths)",
        "the default user's home directory",
        "difference",
        "E2B's template user is `user` with home /home/user; a Cua Space's default user is `cua` with home /home/cua. Paths printed by the guest (`pwd`, `$HOME`) and relative paths resolve under it; code should use `~`/`$HOME` or `files.exists` rather than a hard-coded /home/user.",
        "tests/test_command_errors.py::test_relative_cwd_is_relative_to_the_home",
    ),
    (
        "files.watch_dir / watch_dir events",
        "a file moved out of the watched directory",
        "difference",
        "E2B reports the old name as RENAME; cua-spacesd's watcher reports a move out of the tree as a REMOVE (a move inside the tree is folded to E2B's RENAME + CREATE), indistinguishable from `rm`.",
        "tests/test_watch_fidelity.py::test_a_real_remove_and_a_rename_without_origin_stay_as_they_are",
    ),
]
