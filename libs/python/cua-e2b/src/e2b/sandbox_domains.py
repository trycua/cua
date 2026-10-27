"""Domains whose sandboxes share one `sandbox.<domain>` envd endpoint on E2B."""

SUPPORTED_SANDBOX_DOMAINS = ("e2b.app", "e2b.dev", "e2b.pro", "e2b-staging.dev")


def is_supported_sandbox_domain(sandbox_domain: str) -> bool:
    return sandbox_domain in SUPPORTED_SANDBOX_DOMAINS
