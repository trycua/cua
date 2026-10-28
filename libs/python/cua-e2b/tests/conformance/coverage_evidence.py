"""Which test proves which part of the public surface: `(glob over the name
path, references)`. The longest matching glob wins. A reference is
`tests/<file>.py::<test>` (or `tests/<file>.py` for the whole module), or
`conformance:<case>` for a case of the shared conformance suite, which is
checked against behaviour recorded from the real service
(`tests/conformance/golden/e2b-<version>-behavior.json`).
`tests/test_coverage_matrix.py` checks that every reference exists and that
every implemented class has one. Name, signature, field and base-class
equality with each real release is proven for all names by
`tests/test_surface.py`; this map adds the behavioural proof.
"""

EVIDENCE = [
    # -- sandbox: async and sync ------------------------------------------------------
    (
        "e2b.sandbox_async.main.*",
        "tests/test_e2b_semantics.py; tests/test_runtime_fidelity.py; conformance:get_info; conformance:set_timeout; conformance:kill_twice; conformance:pause_connect; conformance:snapshot_roundtrip; conformance:fork_running",
    ),
    (
        "e2b.sandbox_sync.main.*",
        "tests/test_sync_api.py; tests/test_sync_api.py::test_pagination_and_class_level_calls",
    ),
    (
        "e2b.sandbox_async.sandbox_api.*",
        "tests/test_runtime_fidelity.py::test_listing_pages_with_a_cursor_and_loses_nothing",
    ),
    (
        "e2b.sandbox_sync.sandbox_api.*",
        "tests/test_sync_api.py::test_pagination_and_class_level_calls",
    ),
    (
        "e2b.sandbox_async.paginator.*",
        "tests/test_runtime_fidelity.py::test_listing_pages_with_a_cursor_and_loses_nothing",
    ),
    (
        "e2b.sandbox_sync.paginator.*",
        "tests/test_sync_api.py::test_pagination_and_class_level_calls",
    ),
    ("e2b.paginator.*", "tests/test_config_types.py"),
    (
        "e2b.sandbox.main.*",
        "tests/test_config_types.py; tests/test_runtime_fidelity.py::test_download_and_upload_urls_are_signed_by_spacesd",
    ),
    ("e2b.sandbox.sandbox_api.*", "tests/test_config_types.py; tests/test_e2b_semantics.py"),
    (
        "e2b.sandbox.network.*",
        "tests/test_e2b_semantics.py::test_ip_rules_become_egress_rules_in_the_space",
    ),
    ("e2b.sandbox.iam.*", "tests/test_config_types.py"),
    ("e2b.sandbox.mcp.*", "tests/test_config_types.py"),
    ("e2b.sandbox.signature.*", "tests/test_config_types.py"),
    ("e2b.sandbox.utils.*", "tests/test_e2b_semantics.py; tests/test_sync_api.py"),
    # -- commands, pty, git -----------------------------------------------------------
    (
        "e2b.sandbox_async.commands.*",
        "tests/test_e2b_semantics.py; tests/test_runtime_fidelity.py; tests/test_properties.py; conformance:run_fg; conformance:run_nonzero; conformance:background; conformance:stdin; conformance:pty",
    ),
    (
        "e2b.sandbox_sync.commands.*",
        "tests/test_sync_api.py::test_background_handle_iterates_output_then_ends; tests/test_sync_api.py::test_pty_output_through_the_handle",
    ),
    ("e2b.sandbox.commands.*", "tests/test_e2b_semantics.py; tests/test_properties.py"),
    ("e2b.sandbox_async.git.*", "tests/test_e2b_semantics.py; conformance:git"),
    ("e2b.sandbox_sync.git.*", "tests/test_sync_api.py; conformance:git"),
    # -- filesystem ---------------------------------------------------------------------
    (
        "e2b.sandbox_async.filesystem.*",
        "tests/test_e2b_semantics.py; tests/test_runtime_fidelity.py; conformance:files; conformance:watch_dir; conformance:metadata",
    ),
    (
        "e2b.sandbox_sync.filesystem.*",
        "tests/test_sync_api.py::test_files_round_trip_stream_and_info; tests/test_sync_api.py::test_watch_is_polled_and_stop_is_final",
    ),
    (
        "e2b.sandbox.filesystem.*",
        "tests/test_e2b_semantics.py; tests/test_runtime_fidelity.py; tests/test_properties.py",
    ),
    # -- connection, client, api, secrets ---------------------------------------------
    ("e2b.connection_config.*", "tests/test_config_types.py"),
    ("e2b.client.*", "tests/test_config_types.py"),
    ("e2b.api.*", "tests/test_config_types.py"),
    ("e2b.retry.*", "tests/test_config_types.py"),
    ("e2b.io_utils.*", "tests/test_config_types.py"),
    ("e2b.sandbox_domains.*", "tests/test_config_types.py"),
    ("e2b.exceptions.*", "tests/test_config_types.py; tests/test_e2b_semantics.py"),
    ("e2b.secret.*", "tests/test_secret_errors.py"),
    # -- templates and volumes ------------------------------------------------------------
    ("e2b.template*", "tests/test_template_modules.py; tests/test_e2b_semantics.py"),
    ("e2b.volume*", "tests/test_volume_modules.py"),
]
