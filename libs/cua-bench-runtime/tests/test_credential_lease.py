from __future__ import annotations

import copy
import json
import pickle
import threading
import unittest

from cua_bench_runtime.credential_lease import (
    MAX_SECRET_BYTES,
    CredentialLease,
    CredentialLeaseError,
)


POLICY_DIGEST = "sha256:" + "a" * 64
SECRET = "top-secret-value"


class CredentialLeaseTests(unittest.TestCase):
    def lease(self, **overrides: object) -> CredentialLease:
        arguments = {
            "provider": "openai",
            "harness": "codex",
            "credentials": {"OPENAI_API_KEY": SECRET},
            "expires_at": 200.0,
            "policy_digest": POLICY_DIGEST,
            "clock": lambda: 100.0,
        }
        arguments.update(overrides)
        return CredentialLease(**arguments)  # type: ignore[arg-type]

    def test_consumes_once_and_scrubs_internal_buffer(self) -> None:
        lease = self.lease()
        buffer = lease._buffers["OPENAI_API_KEY"]

        self.assertEqual(
            lease.consume(provider="openai", harness="codex"),
            {"OPENAI_API_KEY": SECRET},
        )
        self.assertEqual(lease.state, "consumed")
        self.assertTrue(buffer)
        self.assertEqual(set(buffer), {0})
        self.assertEqual(lease._buffers, {})
        with self.assertRaisesRegex(CredentialLeaseError, "not active"):
            lease.consume(provider="openai", harness="codex")

    def test_metadata_repr_and_json_are_secret_free(self) -> None:
        lease = self.lease()
        metadata = lease.metadata()

        self.assertEqual(
            set(metadata),
            {"lease_id", "provider", "expires_at", "state", "policy_digest"},
        )
        self.assertEqual(metadata["provider"], "openai")
        self.assertEqual(metadata["state"], "active")
        self.assertNotIn(SECRET, repr(lease))
        self.assertNotIn(SECRET, json.dumps(metadata))
        with self.assertRaises(TypeError):
            json.dumps(lease)
        with self.assertRaises(TypeError):
            hash(lease)

    def test_copy_and_persistence_are_forbidden(self) -> None:
        lease = self.lease()
        for operation in (
            lambda: copy.copy(lease),
            lambda: copy.deepcopy(lease),
            lambda: pickle.dumps(lease),
        ):
            with self.assertRaises(TypeError):
                operation()

    def test_expiry_fails_closed_and_scrubs(self) -> None:
        now = [100.0]
        lease = self.lease(clock=lambda: now[0])
        buffer = lease._buffers["OPENAI_API_KEY"]
        now[0] = 200.0

        with self.assertRaisesRegex(CredentialLeaseError, "not active"):
            lease.consume(provider="openai", harness="codex")

        self.assertEqual(lease.state, "expired")
        self.assertEqual(set(buffer), {0})

    def test_malformed_clock_fails_closed_and_scrubs(self) -> None:
        lease = self.lease(clock=lambda: float("nan"))
        buffer = lease._buffers["OPENAI_API_KEY"]

        with self.assertRaisesRegex(CredentialLeaseError, "invalid value"):
            lease.consume(provider="openai", harness="codex")

        self.assertEqual(set(buffer), {0})
        self.assertEqual(lease._state, "expired")

    def test_binding_mismatch_destroys_lease(self) -> None:
        lease = self.lease()
        with self.assertRaisesRegex(CredentialLeaseError, "binding mismatch"):
            lease.consume(provider="openai", harness="opencode")
        self.assertEqual(lease.state, "destroyed")
        with self.assertRaisesRegex(CredentialLeaseError, "not active"):
            lease.consume(provider="openai", harness="codex")

    def test_supported_provider_harness_pairs_are_explicit(self) -> None:
        cases = (
            ("openai", "codex", "OPENAI_API_KEY"),
            ("openai", "codex", "CDB_CODEX_AUTH_JSON"),
            ("openai", "opencode", "OPENAI_API_KEY"),
            ("anthropic", "claude-code", "ANTHROPIC_API_KEY"),
            ("anthropic", "claude-code", "CLAUDE_CODE_OAUTH_TOKEN"),
            ("anthropic", "opencode", "ANTHROPIC_API_KEY"),
        )
        for provider, harness, name in cases:
            with self.subTest(provider=provider, harness=harness):
                lease = self.lease(
                    provider=provider,
                    harness=harness,
                    credentials={name: SECRET},
                )
                self.assertEqual(lease.consume(provider=provider, harness=harness)[name], SECRET)

    def test_malformed_inputs_and_unsupported_names_are_rejected(self) -> None:
        cases = (
            {"provider": "unknown"},
            {"harness": "unknown"},
            {"credentials": {}},
            {"credentials": {"OPENAI_TOKEN": SECRET}},
            {"credentials": {"OPENAI_API_KEY": ""}},
            {"credentials": {"OPENAI_API_KEY": b"bad\xff"}},
            {"credentials": {"OPENAI_API_KEY": "nul\x00value"}},
            {"credentials": {"OPENAI_API_KEY": "x" * (MAX_SECRET_BYTES + 1)}},
            {"policy_digest": "not-a-digest"},
            {"expires_at": float("inf")},
            {"provider": None},
            {"harness": None},
        )
        for overrides in cases:
            with self.subTest(overrides=tuple(overrides)):
                with self.assertRaises(CredentialLeaseError):
                    self.lease(**overrides)

    def test_concurrent_consumers_allow_exactly_one_success(self) -> None:
        lease = self.lease()
        barrier = threading.Barrier(8)
        results: list[dict[str, str]] = []
        errors: list[Exception] = []

        def consume() -> None:
            barrier.wait()
            try:
                results.append(lease.consume(provider="openai", harness="codex"))
            except Exception as error:  # inspected below
                errors.append(error)

        threads = [threading.Thread(target=consume) for _ in range(8)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()

        self.assertEqual(results, [{"OPENAI_API_KEY": SECRET}])
        self.assertEqual(len(errors), 7)
        self.assertTrue(all(isinstance(error, CredentialLeaseError) for error in errors))


if __name__ == "__main__":
    unittest.main()
