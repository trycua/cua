"""Regenerates coverage.json from the SDK's current surface (run with the pinned daytona installed)."""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import surface

S = {"python": surface.py_surface(), "typescript": surface.ts_surface()}
rows = []


def row(area, py, ts, cases, status="implemented", source="", note="", reason=""):
    r = {"area": area, "py": py, "ts": ts, "status": status, "cases": cases, "source": source}
    if note:
        r["note"] = note
    if reason:
        r["reason"] = reason
    rows.append(r)


def P(cls, *n):
    return [f"{cls}.{x}" for x in n]


SB = "daytona/_sync/sandbox.py, SandboxApi (api client)"
row(
    "Client: create, get, list (labels, states, filters, sort, pages), delete, start, stop, fork",
    P("Daytona", "create", "get", "list", "delete", "start", "stop"),
    P("Daytona", "create", "get", "list", "delete", "start", "stop", "fork"),
    [
        "api.create_params",
        "api.create_defaults",
        "api.create_from_image_resources",
        "api.create_from_image_disk",
        "api.create_name_conflict",
        "api.get_missing",
        "api.auth_bad_key",
        "api.list_labels_and_pages",
        "api.list_filters",
        "api.list_rejects_bad_limit",
        "api.delete",
        "api.fork",
        "api.create_validation_is_client_side",
        "rust:daytona_api::list_filters_sort_and_cursor",
        "rust:daytona_api::create_validation_is_enforced_server_side",
        "rust:daytona_api::create_parameters_reach_the_record_and_the_backend",
    ],
    source="daytona/_sync/daytona.py create/get/list; common/daytona.py CreateSandboxBaseParams; SandboxApi.list_sandboxes",
    note="resources, env_vars, labels, public, auto_*_interval, ttl, volumes, network_block_all/allow_list, secrets, kvm, spot, gpu, ephemeral, user, language, name, linked_sandbox, queue_timeout, target are all read. `cua.on`, `cua.kind` labels place the Space on any machine, kind or OS (Cua extras); labels `cua.secrets=plaintext` opts into secret injection.",
)
row(
    "Client helpers (TypeScript only: HTTP client factory, sub-services)",
    [],
    [
        "Daytona.createAxiosInstance",
        "Daytona.secret",
        "Daytona.snapshot",
        "Daytona.volume",
        "Daytona.warmPool",
    ],
    ["api.create_params"],
    source="Daytona.d.ts",
    note="Client-side only; no wire call of its own.",
)
row(
    "Sandbox: lifecycle (start, stop, pause, archive, recover), waits, refresh",
    P(
        "Sandbox",
        "start",
        "stop",
        "pause",
        "archive",
        "recover",
        "wait_for_sandbox_start",
        "wait_for_sandbox_stop",
        "refresh_data",
        "refresh_activity",
        "delete",
        "fork",
    ),
    P(
        "Sandbox",
        "start",
        "stop",
        "pause",
        "archive",
        "recover",
        "waitUntilStarted",
        "waitUntilStopped",
        "refreshData",
        "refreshActivity",
        "delete",
        "fork",
    ),
    [
        "api.lifecycle_stop_start",
        "api.lifecycle_archive_rules",
        "api.lifecycle_pause",
        "api.lifecycle_recover_only_in_error",
        "api.delete_wait",
        "api.refresh",
        "api.ephemeral",
        "rust:daytona_api::lifecycle_follows_the_documented_rules",
    ],
    source=SB + "; Socket.IO sandbox.state.updated events",
    note="Start/stop map to resume/suspend of the Space (memory kept). Archive keeps the disk on the host (no object-storage tier). States and rules follow the SDK docs; busy states refuse further operations.",
)
row(
    "Sandbox: labels, public flag, auto-stop/pause/archive/delete intervals, ttl",
    P(
        "Sandbox",
        "set_labels",
        "set_public",
        "set_autostop_interval",
        "set_auto_pause_interval",
        "set_auto_archive_interval",
        "set_auto_delete_interval",
        "set_ttl",
    ),
    P(
        "Sandbox",
        "setLabels",
        "setPublic",
        "setAutostopInterval",
        "setAutoPauseInterval",
        "setAutoArchiveInterval",
        "setAutoDeleteInterval",
        "setTtl",
    ),
    ["api.labels_set", "api.intervals", "api.public_flag"],
    source=SB,
)
row(
    "Sandbox: metrics, home/work dirs, env update",
    P(
        "Sandbox",
        "get_metrics",
        "get_metrics_latest",
        "get_user_home_dir",
        "get_user_root_dir",
        "get_work_dir",
        "update_env",
    ),
    P(
        "Sandbox",
        "getMetrics",
        "getMetricsLatest",
        "getUserHomeDir",
        "getUserRootDir",
        "getWorkDir",
        "updateEnv",
    ),
    ["sandbox.metrics", "sandbox.home_and_work_dirs"],
    source="SandboxApi telemetry; InfoApi; ServerApi.update_env",
)
row(
    "Sandbox: preview links, signed preview URLs",
    P("Sandbox", "get_preview_link", "create_signed_preview_url", "expire_signed_preview_url"),
    P("Sandbox", "getPreviewLink", "getSignedPreviewUrl", "expireSignedPreviewUrl"),
    [
        "preview.private_link_needs_its_token",
        "preview.public_link_needs_nothing",
        "preview.signed_url",
        "preview.port_not_open",
    ],
    source="_sync/sandbox.py get_preview_link/create_signed_preview_url",
    note="URLs are https://<port>-<id>.<domain> on the gateway's TLS host router (a Cua tunnel to the guest port, any machine). Private links accept X-Daytona-Preview-Token or the API key; a signed URL carries its token in the host name.",
)
row(
    "Sandbox: signed file URLs",
    P("Sandbox", "download_url", "upload_url", "rotate_signing_key"),
    P("Sandbox", "downloadUrl", "uploadUrl", "rotateSigningKey"),
    ["fs.signed_urls"],
    source="_utils/file_url_signing.py (HMAC-SHA256 over v1:files:<method>:<path>:<expires>)",
)
row(
    "Sandbox: network policy and secrets",
    P("Sandbox", "update_network_settings", "update_secrets"),
    P("Sandbox", "updateNetworkSettings", "updateSecrets"),
    [
        "secret.mount_is_opt_in",
        "rust:daytona_api::network_settings_validate_before_touching_the_guest",
        "rust:daytona_api::volumes_and_secrets_manage_their_objects",
    ],
    source="SandboxApi.update_network_settings/update_sandbox_secrets",
    note="Egress is an nftables policy in the guest (cua_spacesd_client::EgressPolicy); secrets are injected as plaintext env values only with label cua.secrets=plaintext (Daytona substitutes at an egress proxy Cua does not have).",
)
row(
    "Sandbox: create snapshot",
    P("Sandbox", "create_snapshot"),
    P("Sandbox", "createSnapshot"),
    ["snapshot.from_sandbox"],
    source="SandboxApi.create_sandbox_snapshot",
    note="A Cua checkpoint registered under the snapshot name; sandboxes created from it are copy-on-write forks.",
)
row(
    "Sandbox: resize",
    P("Sandbox", "resize", "wait_for_resize_complete"),
    P("Sandbox", "resize", "waitForResizeComplete"),
    [
        "api.resize_rules",
        "api.resize_applies_on_a_real_guest",
        "rust:daytona_api::unbuildable_requests_are_refused_with_their_reason_not_accepted",
    ],
    source="SandboxApi.resize_sandbox; _sync/sandbox.py resize",
    note="The SDK's rules (hot resize only grows cpu/memory, disk grows only while stopped, no gpu) are enforced and the sandbox goes through the `resizing` state with events. The new shape is applied by the core's `Sandbox.resize` (a local VM stops and boots again from its own disk with the new vCPUs, memory and a larger disk; files are kept, running processes are not); a sandbox that was stopped stays stopped. Placements that cannot be resized (containers, clouds) answer Daytona's 400 with the core's reason.",
)
row(
    "Sandbox: SSH access",
    P("Sandbox", "create_ssh_access", "validate_ssh_access", "revoke_ssh_access"),
    P("Sandbox", "createSshAccess", "validateSshAccess", "revokeSshAccess"),
    ["ssh.access_token_lifecycle", "ssh.login_through_the_endpoint"],
    source="SandboxApi.create_ssh_access/validate_ssh_access/revoke_ssh_access",
    note="A time-limited loopback endpoint on the gateway host piped to the guest's sshd through the Space's tunnel forward, with a one-time key authorized for the sandbox user and removed on revoke or expiry. `sshCommand` names the port and key file, so it works on the gateway's machine. Images without sshd get a 400 naming that.",
)
row(
    "Sandbox: members",
    P(
        "Sandbox",
        "code_interpreter",
        "computer_use",
        "create_lsp_server",
        "fs",
        "git",
        "process",
        "model_post_init",
    ),
    P(
        "Sandbox",
        "codeInterpreter",
        "computerUse",
        "createLspServer",
        "fs",
        "git",
        "process",
        "cpu",
        "disk",
        "gpu",
        "id",
        "labels",
        "memory",
        "name",
        "organizationId",
        "public",
        "target",
        "toolboxProxyUrl",
        "user",
    ),
    ["api.create_params"],
    source="Sandbox.d.ts / sandbox.py",
    note="Accessors and DTO fields; covered through the rows of the services they return and by the DTO checks of api.create_params.",
)
row(
    "Process: exec, code_run (python/javascript/typescript, argv, env, timeout), exit codes, timeout error",
    P("Process", "exec", "code_run"),
    P("Process", "executeCommand", "codeRun"),
    [
        "process.exec_result_and_exit_code",
        "process.exec_timeout_error",
        "process.exec_cwd_env_user",
        "process.exec_shell_semantics",
        "process.exec_large_output",
        "process.code_run_python",
        "process.code_run_timeout",
        "process.code_run_charts",
        "rust:daytona_toolbox::a_command_past_its_timeout_is_a_408_with_the_daemon_code",
    ],
    source="_sync/process.py exec/code_run; common/errors.py PROCESS_EXECUTION_TIMEOUT",
)
row(
    "Process: sessions (create, execute, async, logs incl. follow, input, ordering, delete)",
    P(
        "Process",
        "create_session",
        "get_session",
        "get_session_command",
        "execute_session_command",
        "get_session_command_logs",
        "get_session_command_logs_async",
        "send_session_command_input",
        "list_sessions",
        "delete_session",
        "get_entrypoint_session",
        "get_entrypoint_logs",
        "get_entrypoint_logs_async",
    ),
    P(
        "Process",
        "createSession",
        "getSession",
        "getSessionCommand",
        "executeSessionCommand",
        "getSessionCommandLogs",
        "sendSessionCommandInput",
        "listSessions",
        "deleteSession",
        "getEntrypointSession",
        "getEntrypointLogs",
    ),
    [
        "process.session_state_carries_over",
        "process.session_async_command_and_logs",
        "process.session_follow_logs",
        "process.session_input_to_running_command",
        "process.session_command_ordering",
        "process.session_errors",
        "rust:daytona_toolbox::statuses_follow_the_generated_clients",
    ],
    source="_sync/process.py",
    note="cwd and exports carry over between commands of a session; commands run in order; the entrypoint session is empty (Cua images have no Daytona entrypoint).",
)
row(
    "Process: PTY (create, connect, resize, list, info, kill)",
    P(
        "Process",
        "create_pty_session",
        "connect_pty_session",
        "list_pty_sessions",
        "get_pty_session_info",
        "resize_pty_session",
        "kill_pty_session",
    ),
    P(
        "Process",
        "createPty",
        "connectPty",
        "listPtySessions",
        "getPtySessionInfo",
        "resizePtySession",
        "killPtySession",
    ),
    ["process.pty_roundtrip", "process.pty_resize_list_kill"],
    source="_sync/process.py; handle/pty_handle.py (WebSocket)",
)
row(
    "FileSystem: single and bulk transfer, streaming, metadata, search/replace, permissions",
    P("FileSystem", *S["python"]["FileSystem"]),
    P("FileSystem", *S["typescript"]["FileSystem"]),
    [
        "fs.roundtrip_binary_and_empty",
        "fs.unicode_and_space_names",
        "fs.upload_creates_parents_and_overwrites",
        "fs.list_info_folder_move_delete",
        "fs.errors_carry_file_codes",
        "fs.bulk_upload_download",
        "fs.download_to_local_path_and_stream",
        "fs.upload_stream",
        "fs.concurrent_transfers",
        "fs.large_file_integrity",
        "fs.permissions_and_owner",
        "fs.find_search_replace",
        "rust:daytona_toolbox::files_report_their_own_codes_and_bulk_parts_are_matched_by_filename_star",
        "rust:daytona_toolbox::a_move_does_not_replace_its_destination",
    ],
    source="_sync/filesystem.py; common/file_transfer.py",
)
row(
    "Code interpreter (contexts, streaming, errors, timeout)",
    P("CodeInterpreter", *S["python"]["CodeInterpreter"]),
    P("CodeInterpreter", *S["typescript"]["CodeInterpreter"]),
    [
        "interpreter.default_context_keeps_state",
        "interpreter.contexts_are_isolated",
        "interpreter.streams_stdout_stderr_and_errors",
        "interpreter.envs_and_exit",
        "interpreter.timeout",
    ],
    source="_sync/code_interpreter.py (WebSocket protocol); Python only (the interpreter is Python, other languages use process.code_run)",
)
row(
    "LSP servers (python, typescript, javascript)",
    P("LspServer", *S["python"]["LspServer"]),
    P("LspServer", *S["typescript"]["LspServer"]),
    ["lsp.python_session", "lsp.errors"],
    source="_sync/lsp_server.py; LspApi",
    note="pylsp and typescript-language-server run in the guest, installed on first start when the guest has network; otherwise Daytona's LSP_SERVER_NOT_INITIALIZED error names what is missing.",
)
row(
    "Computer use: processes",
    P("ComputerUse", *S["python"]["ComputerUse"]),
    P("ComputerUse", *S["typescript"]["ComputerUse"]),
    [
        "computer.start_status_and_processes",
        "rust:daytona_computer::displays_windows_and_the_desktop_processes",
    ],
    source="_sync/computer_use.py ComputerUse",
    note="Linux Spaces report xvfb/xfce4/x11vnc/novnc as running when the Cua desktop session is up; macOS and Windows Spaces report `desktop`. `stop` answers that the desktop is the Space's own and keeps running.",
)
row(
    "Computer use: mouse, keyboard, screenshot, display, windows",
    P("Mouse", *S["python"]["Mouse"])
    + P("Keyboard", *S["python"]["Keyboard"])
    + P("Screenshot", *S["python"]["Screenshot"])
    + P("Display", *S["python"]["Display"]),
    P("Mouse", *S["typescript"]["Mouse"])
    + P("Keyboard", *S["typescript"]["Keyboard"])
    + P("Screenshot", *S["typescript"]["Screenshot"])
    + P("Display", *S["typescript"]["Display"]),
    [
        "computer.mouse",
        "computer.keyboard",
        "computer.screenshots",
        "computer.display_and_windows",
        "computer.real_desktop",
        "rust:daytona_computer::pointer_and_keyboard_requests_become_computer_service_actions",
        "rust:daytona_computer::screenshots_ask_for_the_requested_region_format_and_scale",
    ],
    source="_sync/computer_use.py; computer_use_api.py",
    note="Runs on Linux, macOS and Windows Spaces through cua-spacesd's computer service (Cua extra).",
)
row(
    "Computer use: recordings",
    P("RecordingService", *S["python"]["RecordingService"]),
    P("RecordingService", *S["typescript"]["RecordingService"]),
    ["computer.recording"],
    source="_sync/computer_use.py RecordingService",
    note="MP4 from the desktop's frames (cua_spacesd_client::record_screen, ffmpeg on the gateway host); RECORDING_FFMPEG_NOT_FOUND (503) without ffmpeg.",
)
row(
    "Computer use: accessibility",
    P("Accessibility", *S["python"]["Accessibility"]),
    P("Accessibility", *S["typescript"]["Accessibility"]),
    [
        "computer.accessibility",
        "rust:daytona_computer::accessibility_nodes_are_addressed_by_snapshot_and_element",
    ],
    source="_sync/computer_use.py Accessibility",
    note="Cua's accessibility service (AX on macOS, UIA on Windows, AT-SPI on Linux).",
)
row(
    "Snapshots",
    P("SnapshotService", *S["python"]["SnapshotService"]),
    P("SnapshotService", *S["typescript"]["SnapshotService"]),
    [
        "snapshot.create_list_get_delete",
        "snapshot.pagination",
        "snapshot.duplicate_and_missing",
        "snapshot.activate_deactivate",
        "snapshot.from_sandbox",
        "snapshot.build_from_image_object",
        "snapshot.build_failure_is_reported",
        "rust:daytona_api::snapshots_pull_build_and_report_failures",
    ],
    source="_sync/snapshot.py; SnapshotsApi",
    note="From an image reference (pulled/validated), a Dockerfile (built into Cua image layers, build log streamed), or a sandbox (checkpoint).",
)
row(
    "Volumes",
    P("VolumeService", *S["python"]["VolumeService"]),
    P("VolumeService", *S["typescript"]["VolumeService"]),
    [
        "volume.crud",
        "volume.mount_needs_cua_volume",
        "volume.mount_without_a_grant_is_explained",
        "volume.shared_between_sandboxes",
        "rust:daytona_api::volumes_and_secrets_manage_their_objects",
    ],
    source="_sync/volume.py; VolumesApi",
    note="A volume is a folder of Cua Volume bound at the mount path (subpath supported); needs Cua Volume mounted in the Space.",
)
row(
    "Secrets",
    P("SecretService", *S["python"]["SecretService"]),
    P("SecretService", *S["typescript"]["SecretService"]),
    ["secret.crud", "secret.mount_is_opt_in"],
    source="_sync/secret.py; SecretApi",
)
row(
    "Warm pools",
    P("WarmPoolService", *S["python"]["WarmPoolService"]),
    P("WarmPoolService", *S["typescript"]["WarmPoolService"]),
    ["warm_pool.crud", "rust:daytona_api::warm_pools_map_to_cua_pools"],
    source="_sync/warm_pool.py; WarmPoolsApi",
    note="Cua pools (Sandboxes.create_pool): image snapshots only, on this machine.",
)
row(
    "Image builder (Dockerfile subset, local file/dir contexts)",
    P("Image", *S["python"]["Image"]),
    P("Image", *[x for x in S["typescript"]["Image"]]),
    [
        "image.create_with_layers",
        "image.context_upload",
        "image.unbuildable_dockerfile_is_refused",
        "rust:daytona_api::build_info_becomes_an_image_or_layers",
        "rust:daytona_api::the_object_store_takes_the_uploads_both_sdks_make",
    ],
    source="common/image.py; _sync/object_storage.py",
    note="FROM/RUN/ENV/WORKDIR/USER/COPY/ADD/EXPOSE/ENTRYPOINT/CMD/ARG; multi-stage, ONBUILD, --mount and here-documents are refused naming the instruction. The context upload goes to an S3-compatible endpoint on the gateway.",
)
GIT = [
    "git.init_branches",
    "git.repo_errors",
    "git.clone_depth_commit",
    "git.http_auth",
    "git.commit_status_flow",
    "git.push_pull_roundtrip",
    "git.push_rejected_and_conflict",
    "git.branch_lifecycle",
    "git.dirty_checkout",
    "git.reset_restore",
    "git.remotes",
    "git.config",
    "git.history_route",
    "rust:daytona_git",
]
row(
    "Git (init, clone, add, commit, push, pull, status, branches, checkout, reset, restore, remotes, config, credentials)",
    P("Git", *S["python"]["Git"]),
    P("Git", *S["typescript"]["Git"]),
    GIT,
    source="_sync/git.py; GitApi",
    note="Over the guest git CLI; credentials are passed by a one-shot askpass helper and never reach argv or .git/config. Errors map to the daemon's GIT_* codes (see differences-daytona.md).",
)
row(
    "Image processing helper",
    P("SnapshotService", "process_image_context"),
    P("SnapshotService", "processImageContext"),
    ["image.context_upload"],
    source="_sync/snapshot.py process_image_context",
)
row(
    "Image: git-like members of the TypeScript image",
    [],
    ["Image.contextList"],
    ["image.create_with_layers"],
    source="Image.d.ts",
)

# Interim gateway pieces that belong to a core platform system (COORD.md
# "Platform systems"); they are not extended, and move when that system lands.
INTERIM = {
    "Sandbox: network policy and secrets": "network policy: Space Policy + host-enforced egress; secrets: egress gateway + secrets broker",
    "Sandbox: create snapshot": "power states + snapshot store",
    "Code interpreter": "the platform code interpreter",
    "Snapshots": "the image build service (Spaces.build) and the snapshot store",
    "Volumes": "platform volumes (named volumes)",
    "Secrets": "the egress gateway + secrets broker",
    "Warm pools": "the elastic pool's prewarm (daytona/prewarm.rs resolves the Daytona side)",
    "Image builder (Dockerfile subset, local file/dir contexts)": "the image build service (Spaces.build)",
    "Sandbox: metrics, home/work dirs, env update": "observability (metrics)",
}
for r in rows:
    for k, v in INTERIM.items():
        if r["area"] == k or r["area"].startswith(k + " "):
            r["note"] = (r.get("note", "") + f" INTERIM: moves to {v}; not extended here.").strip()
json.dump({"sdk": "0.225.0", "rows": rows}, open("coverage.json", "w"), indent=1)
