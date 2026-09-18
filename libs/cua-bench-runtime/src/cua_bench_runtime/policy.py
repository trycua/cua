"""Benchmark-owned execution-policy accounting and eligibility receipts."""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Mapping
from decimal import Decimal, InvalidOperation
import math
import re
from typing import Any

from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.errors import BudgetExceeded, ValidationFailure


TOKEN_FIELDS = ("input", "output", "cache_read", "cache_write")
COMPARISON_ONLY_VIOLATIONS = frozenset(
    {"model_telemetry_unavailable", "model_telemetry_not_certifying"}
)
_DIGEST = re.compile(r"^sha256:[a-f0-9]{64}$")
_PROVIDER_PROXY_FIELDS = frozenset(
    {
        "enforcer",
        "schema_version",
        "trial_id",
        "endpoint",
        "allowed_client_ip",
        "provider_allowlist_sha256",
        "initial_client_binding_digest",
        "sealed_client_binding_digest",
        "initial_implementation_digest",
        "sealed_implementation_digest",
        "implementation_identity",
        "initial_evidence_digest",
        "sealed_evidence_digest",
        "artifact_sha256",
        "accepted_connections",
        "rejected_connections",
        "bytes_guest_to_provider",
        "bytes_provider_to_guest",
        "transcript_chain_digest",
        "active",
        "sealed",
    }
)


def certification_integrity(receipt: Mapping[str, Any] | None) -> dict[str, Any]:
    """Separate execution-integrity policy failures from model observability."""

    if not isinstance(receipt, Mapping):
        return {"passed": False, "violations": ["execution_policy_receipt_unavailable"]}
    violations = receipt.get("violations")
    if not isinstance(violations, list) or any(
        not isinstance(value, str) or not value for value in violations
    ):
        return {"passed": False, "violations": ["execution_policy_receipt_invalid"]}
    integrity = sorted(
        value for value in set(violations) if value not in COMPARISON_ONLY_VIOLATIONS
    )
    return {"passed": not integrity, "violations": integrity}


class ExecutionPolicyController:
    """Collect trusted adapter facts and enforce externally owned budgets.

    Harness adapters call ``record_model_call`` from their provider boundary.
    A generic subprocess adapter cannot certify model routing and therefore
    produces an unavailable receipt instead of trusting agent-authored files.
    """

    def __init__(
        self,
        *,
        trial_id: str,
        system: Mapping[str, Any],
        policy: Mapping[str, Any],
        model_price_table: Mapping[str, Any],
        system_digest: str,
        policy_digest: str,
    ) -> None:
        self.trial_id = trial_id
        self.system = system
        self.policy = policy
        self.model_price_table = model_price_table
        self.system_digest = system_digest
        self.policy_digest = policy_digest
        self._declared = {
            route["id"]: {
                key: route[key]
                for key in (
                    "role",
                    "provider",
                    "model",
                    "snapshot",
                    "service_tier",
                )
            }
            for route in system["model_routing"]["routes"]
        }
        if policy["accounting"]["cost_basis"] == "pinned_list_price":
            if model_price_table.get("currency") != policy["accounting"]["currency"]:
                raise ValidationFailure("model price table currency mismatch")
            for route in system["model_routing"]["routes"]:
                model_key = f"{route['model']}@{route['snapshot']}"
                prices = model_price_table.get("models", {}).get(model_key)
                if not isinstance(prices, Mapping):
                    raise ValidationFailure(f"model price table omits declared route {model_key}")
                for field in ("input", "output"):
                    value = prices.get(f"{field}_per_million")
                    if (
                        not isinstance(value, (int, float))
                        or isinstance(value, bool)
                        or not math.isfinite(value)
                        or value < 0
                    ):
                        raise ValidationFailure(
                            f"model price table has invalid {field} price for {model_key}"
                        )
        self._calls: dict[tuple[str, str, str, str, str, str], dict[str, Any]] = {}
        self._tokens: defaultdict[str, int] = defaultdict(int)
        self._cost_usd = Decimal("0")
        self._token_telemetry_complete = True
        self._token_scope_observations: list[bool | None] = []
        self._cost_telemetry_complete = True
        self._human_interventions = 0
        self._approval_prompts = 0
        self._max_parallelism = 1
        self._package_installations: list[dict[str, str]] = []
        self._self_modifications: list[dict[str, str]] = []
        self._violations: list[str] = []
        self._telemetry_trust = "unavailable"

    def record_model_call(
        self,
        *,
        route_id: str,
        role: str,
        provider: str,
        model: str,
        snapshot: str,
        service_tier: str,
        tokens: Mapping[str, int] | None,
        cost_usd: float | None,
        trust: str,
        includes_subagents: bool | None = None,
    ) -> None:
        if trust not in {"certifying", "non_certifying"}:
            raise ValueError("model-call trust must be certifying or non_certifying")
        if includes_subagents is not None and not isinstance(includes_subagents, bool):
            raise ValueError("includes_subagents must be a boolean or None")
        self._telemetry_trust = (
            "certifying"
            if trust == "certifying" and self._telemetry_trust != "non_certifying"
            else "non_certifying"
        )
        identity = {
            "role": role,
            "provider": provider,
            "model": model,
            "snapshot": snapshot,
            "service_tier": service_tier,
        }
        if self._declared.get(route_id) != identity:
            self._add_violation("undeclared_or_mismatched_model_route")
        normalized_tokens: dict[str, int] | None = None
        if tokens is None:
            self._token_telemetry_complete = False
        else:
            if not isinstance(includes_subagents, bool):
                self._token_telemetry_complete = False
            normalized_tokens = {}
            for field in TOKEN_FIELDS:
                value = tokens.get(field)
                if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                    raise ValueError(f"invalid {field} token count")
                normalized_tokens[field] = value
                self._tokens[field] += value
        observed_scope = includes_subagents if normalized_tokens is not None else None
        self._token_scope_observations.append(observed_scope)
        reported_cost: Decimal | None = None
        if cost_usd is not None:
            if (
                not isinstance(cost_usd, (int, float))
                or isinstance(cost_usd, bool)
                or not math.isfinite(cost_usd)
                or cost_usd < 0
            ):
                raise ValueError("invalid model-call cost")
            reported_cost = Decimal(str(cost_usd))
        if self.policy["accounting"]["cost_basis"] == "pinned_list_price":
            if normalized_tokens is None:
                charged_cost = None
            else:
                model_key = f"{model}@{snapshot}"
                try:
                    prices = self.model_price_table["models"][model_key]
                    calculated_cost = Decimal("0")
                    for field in TOKEN_FIELDS:
                        price_key = f"{field}_per_million"
                        if normalized_tokens[field] and price_key not in prices:
                            raise ValueError(
                                f"model price table omits {field} price for {model_key}"
                            )
                        calculated_cost += (
                            Decimal(normalized_tokens[field])
                            * Decimal(str(prices.get(price_key, 0)))
                            / Decimal(1_000_000)
                        )
                except (InvalidOperation, KeyError, TypeError, ValueError) as error:
                    raise ValueError(
                        f"model price table has no valid route for {model_key}"
                    ) from error
                if reported_cost is not None and reported_cost != calculated_cost:
                    self._add_violation("reported_cost_mismatch")
                charged_cost = calculated_cost
        else:
            charged_cost = reported_cost
        if charged_cost is None:
            self._cost_telemetry_complete = False
        else:
            self._cost_usd += charged_cost
        key = (route_id, role, provider, model, snapshot, service_tier)
        entry = self._calls.setdefault(
            key,
            {
                "route_id": route_id,
                **identity,
                "calls": 0,
                "tokens": (
                    {field: 0 for field in TOKEN_FIELDS} if normalized_tokens is not None else None
                ),
            },
        )
        entry["calls"] += 1
        if normalized_tokens is None:
            entry["tokens"] = None
        elif entry["tokens"] is not None:
            for field, value in normalized_tokens.items():
                entry["tokens"][field] += value
        self._enforce_model_budgets()

    def record_human_intervention(self, *, approval_prompt: bool = False) -> None:
        self._human_interventions += 1
        if approval_prompt:
            self._approval_prompts += 1

    def record_parallelism(self, active: int) -> None:
        if active < 1:
            raise ValueError("active parallelism must be positive")
        self._max_parallelism = max(self._max_parallelism, active)

    def record_package_installation(self, name: str, version: str) -> None:
        self._package_installations.append(
            {"name": name, "version": version, "scope": "ephemeral_workspace"}
        )

    def record_self_modification(self, target: str, description: str) -> None:
        self._self_modifications.append({"target": target, "description": description})

    def record_debug_mode(self) -> None:
        """Make diagnostic runs explicitly ineligible for certification/comparison."""

        self._add_violation("debug_mode")

    def _add_violation(self, value: str) -> None:
        if value not in self._violations:
            self._violations.append(value)

    def _enforce_model_budgets(self) -> None:
        if (
            self._token_telemetry_complete
            and self._tokens["input"] + self._tokens["output"]
            > self.policy["limits"]["total_tokens"]
        ):
            self._add_violation("token_limit_exceeded")
            raise BudgetExceeded("model token budget exceeded")
        if self._cost_telemetry_complete and self._cost_usd > Decimal(
            str(self.policy["limits"]["cost_usd"])
        ):
            self._add_violation("cost_limit_exceeded")
            raise BudgetExceeded("model cost budget exceeded")

    def finalize(
        self,
        *,
        config_digest: str,
        elapsed_ms: int,
        environment_facts: Mapping[str, Any] | None,
    ) -> dict[str, Any]:
        facts = dict(environment_facts or {})
        includes_subagents = (
            None
            if not self._token_scope_observations
            or any(value is None for value in self._token_scope_observations)
            else all(self._token_scope_observations)
        )
        if (
            not self._calls
            or not self._token_telemetry_complete
            or includes_subagents is not True
            or not self._cost_telemetry_complete
        ):
            self._add_violation("model_telemetry_unavailable")
        if self._telemetry_trust != "certifying":
            self._add_violation("model_telemetry_not_certifying")
        if self._human_interventions > self.policy["autonomy"]["max_human_interventions"]:
            self._add_violation("human_intervention_exceeded")
        if self._approval_prompts:
            self._add_violation("approval_prompt_observed")
        if elapsed_ms > self.policy["limits"]["wall_time_ms"]:
            self._add_violation("wall_time_limit_exceeded")

        required_facts = {
            "fresh_harness_workspace": True,
            "target_reset": True,
            "applied_network_mode": self.policy["network"]["mode"],
            "applied_permission_policy_sha256": self.policy["permissions"]["policy_sha256"],
            "credential_state_profile_sha256": self.policy["credential_state_profile"]["sha256"],
        }
        for name, expected in required_facts.items():
            if facts.get(name) != expected:
                self._add_violation(f"{name}_unverified")
        expected_allowlist = self.policy["network"].get("allowlist_sha256")
        if facts.get("applied_network_allowlist_sha256") != expected_allowlist:
            self._add_violation("applied_network_allowlist_unverified")
        expected_cache = (
            "empty"
            if self.policy["isolation"]["persistent_cache"] == "forbidden"
            else "declared_read_only"
        )
        if facts.get("cache_state") != expected_cache:
            self._add_violation("cache_state_unverified")
        expected_persistent = (
            "absent"
            if self.policy["isolation"]["persistent_cache"] == "forbidden"
            else "declared_read_only"
        )
        if facts.get("persistent_state") != expected_persistent:
            self._add_violation("persistent_state_unverified")
        enforcement = facts.get("enforcement")
        if facts.get("apparatus_enforcement_required") is True:
            if not isinstance(enforcement, Mapping):
                self._add_violation("apparatus_enforcement_unverified")
            else:
                expected_enforcers = {
                    "network": "guest-root-pf-anchor",
                    "human_input": "host-lume-vnc-disabled",
                    "privileged_helper": "guest-root-cdb-helper",
                }
                for namespace, expected in expected_enforcers.items():
                    evidence = enforcement.get(namespace)
                    if (
                        not isinstance(evidence, Mapping)
                        or evidence.get("enforcer") != expected
                        or not isinstance(evidence.get("evidence_digest"), str)
                        or not re.fullmatch(r"sha256:[a-f0-9]{64}", evidence["evidence_digest"])
                    ):
                        self._add_violation(f"{namespace}_enforcement_unverified")
                binary = enforcement.get("lume_binary")
                if (
                    not isinstance(binary, Mapping)
                    or not isinstance(binary.get("sha256"), str)
                    or not re.fullmatch(r"sha256:[a-f0-9]{64}", binary["sha256"])
                ):
                    self._add_violation("lume_binary_unverified")
                proxy_endpoint = self.policy["network"].get("proxy_endpoint")
                provider_allowlist_sha256 = self.policy["network"].get("provider_allowlist_sha256")
                proxy_implementation_sha256 = self.policy["network"].get(
                    "proxy_implementation_sha256"
                )
                if (
                    proxy_endpoint is not None
                    or provider_allowlist_sha256 is not None
                    or proxy_implementation_sha256 is not None
                ):
                    proxy = enforcement.get("provider_proxy")
                    endpoint_url = (
                        "http://" + proxy_endpoint.replace("@", ":")
                        if isinstance(proxy_endpoint, str)
                        else None
                    )
                    if (
                        not isinstance(proxy, Mapping)
                        or set(proxy) != _PROVIDER_PROXY_FIELDS
                        or proxy.get("enforcer") != "host-cdb-connect-proxy"
                        or proxy.get("schema_version") != 1
                        or proxy.get("trial_id") != self.trial_id
                        or proxy.get("endpoint") != endpoint_url
                        or proxy.get("provider_allowlist_sha256")
                        != "sha256:" + str(provider_allowlist_sha256)
                        or proxy.get("sealed_implementation_digest")
                        != "sha256:" + str(proxy_implementation_sha256)
                        or proxy.get("active") is not False
                        or proxy.get("sealed") is not True
                        or proxy.get("implementation_identity") != "cb.provider-connect-proxy/v2"
                        or any(
                            not isinstance(proxy.get(field), str)
                            or _DIGEST.fullmatch(proxy[field]) is None
                            for field in (
                                "initial_client_binding_digest",
                                "sealed_client_binding_digest",
                                "initial_implementation_digest",
                                "sealed_implementation_digest",
                                "initial_evidence_digest",
                                "sealed_evidence_digest",
                                "artifact_sha256",
                                "transcript_chain_digest",
                            )
                        )
                    ):
                        self._add_violation("provider_proxy_enforcement_unverified")
                    else:
                        if (
                            proxy["initial_client_binding_digest"]
                            != proxy["sealed_client_binding_digest"]
                            or proxy["initial_implementation_digest"]
                            != proxy["sealed_implementation_digest"]
                        ):
                            self._add_violation("provider_proxy_binding_changed")
                        counters_valid = all(
                            isinstance(proxy.get(field), int)
                            and not isinstance(proxy[field], bool)
                            and proxy[field] >= 0
                            for field in (
                                "accepted_connections",
                                "rejected_connections",
                                "bytes_guest_to_provider",
                                "bytes_provider_to_guest",
                            )
                        )
                        if (
                            not counters_valid
                            or proxy["accepted_connections"] < 1
                            or proxy["bytes_guest_to_provider"] < 1
                            or proxy["bytes_provider_to_guest"] < 1
                        ):
                            self._add_violation("provider_proxy_provider_connection_unverified")
        if not facts.get("applications"):
            self._add_violation("applications_unverified")
        display = facts.get("display")
        if not isinstance(display, Mapping) or not all(
            isinstance(display.get(field), (int, float))
            and not isinstance(display[field], bool)
            and math.isfinite(display[field])
            and display[field] > 0
            for field in ("width_px", "height_px", "scale")
        ):
            self._add_violation("display_unverified")
        if (
            self.policy["isolation"]["package_installation"] == "forbidden"
            and self._package_installations
        ):
            self._add_violation("package_installation_observed")
        if self._self_modifications:
            self._add_violation("self_modification_observed")

        status = (
            "passed"
            if not self._violations
            else (
                "unavailable"
                if any(
                    value.endswith("_unavailable") or value.endswith("_unverified")
                    for value in self._violations
                )
                else "failed"
            )
        )
        body = {
            "status": status,
            "eligible": status == "passed",
            "trust": self._telemetry_trust,
            "violations": sorted(self._violations),
            "bindings": {
                "trial_id": self.trial_id,
                "system_digest": self.system_digest,
                "execution_policy_digest": self.policy_digest,
                "config_digest": config_digest,
            },
            "observed": {
                "model_calls": sorted(self._calls.values(), key=lambda item: item["route_id"]),
                "tokens": (
                    {
                        **{field: self._tokens[field] for field in TOKEN_FIELDS},
                        "includes_subagents": includes_subagents,
                    }
                    if self._calls
                    and self._token_telemetry_complete
                    and includes_subagents is not None
                    else None
                ),
                "cost_usd": (
                    float(self._cost_usd) if self._calls and self._cost_telemetry_complete else None
                ),
                "human_interventions": self._human_interventions,
                "approval_prompts": self._approval_prompts,
                "max_parallelism": self._max_parallelism,
                "fresh_harness_workspace": facts.get("fresh_harness_workspace"),
                "target_reset": facts.get("target_reset"),
                "cache_state": facts.get("cache_state", "unknown"),
                "persistent_state": facts.get("persistent_state", "unknown"),
                "package_installations": self._package_installations,
                "applied_network_mode": facts.get("applied_network_mode", "unknown"),
                "applied_network_allowlist_sha256": facts.get("applied_network_allowlist_sha256"),
                "applied_permission_policy_sha256": facts.get("applied_permission_policy_sha256"),
                "credential_state_profile_sha256": facts.get("credential_state_profile_sha256"),
                "self_modifications": self._self_modifications,
                "applications": facts.get("applications", []),
                "display": facts.get("display"),
                "enforcement": enforcement,
                "cost_basis": self.policy["accounting"]["cost_basis"],
                "model_price_table_sha256": self.policy["accounting"]["model_price_table"][
                    "sha256"
                ],
            },
        }
        return {**body, "receipt_digest": digest_json(body)}
