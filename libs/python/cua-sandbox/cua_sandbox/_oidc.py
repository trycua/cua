"""OIDC token helper — exchange a CUA sandbox OIDC token for cloud credentials.

CUA sandboxes can obtain short-lived JWTs from the CUA OIDC issuer
(https://oidc.cua.ai) and exchange them with cloud providers for
temporary credentials — no long-lived secrets stored anywhere.

This mirrors exactly how GitHub Actions / Claude Code work:
  1. Call ``oidc_token()`` → get a short-lived CUA JWT (default 5 min)
  2. Pass it to ``aws_credentials()`` → get short-lived AWS STS credentials
  3. Use those credentials to call Bedrock, S3, etc.

The JWT can also be used directly with GCP Workload Identity Federation
and Azure Managed Identity (federated credentials).

Example — AWS Bedrock with zero stored secrets::

    import cua_sandbox as cua
    import boto3

    # 1. Get a CUA OIDC token for this sandbox
    token = await cua.oidc_token(vm_name="my-agent", audience="sts.amazonaws.com")

    # 2. Exchange with AWS STS for short-lived Bedrock credentials
    creds = await cua.aws_credentials(
        role_arn="arn:aws:iam::123456789012:role/cua-bedrock-role",
        token=token,
    )

    # 3. Use the credentials
    bedrock = boto3.client("bedrock-runtime", **creds.to_boto3_kwargs())

AWS IAM trust policy required (one-time setup)::

    {
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Principal": {"Federated": "arn:aws:iam::123456789012:oidc-provider/oidc.cua.ai"},
        "Action": "sts:AssumeRoleWithWebIdentity",
        "Condition": {
          "StringEquals": {
            "oidc.cua.ai:aud": "sts.amazonaws.com",
            "oidc.cua.ai:sub": "sandbox:workspace:acme:vm:my-agent"
          }
        }
      }]
    }
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any, Dict, Optional

import httpx

from cua_sandbox._config import get_api_key, get_base_url

# How many seconds before JWT expiry to refresh proactively.
_REFRESH_BUFFER_SECS = 30


@dataclass
class OIDCToken:
    """A short-lived JWT issued by the CUA OIDC provider (https://oidc.cua.ai).

    Attributes:
        token: The raw JWT string. Pass this to ``aws_credentials()`` or
            directly to STS ``AssumeRoleWithWebIdentity``.
        expires_in: Token lifetime in seconds from the time of issuance.
        issued_at: Unix timestamp when the token was issued.
    """

    token: str
    expires_in: int
    issued_at: float

    @property
    def expires_at(self) -> float:
        return self.issued_at + self.expires_in

    @property
    def is_expired(self) -> bool:
        return time.time() >= self.expires_at - _REFRESH_BUFFER_SECS

    def __str__(self) -> str:
        return self.token


@dataclass
class AWSCredentials:
    """Temporary AWS credentials obtained by exchanging a CUA OIDC token with STS.

    Attributes:
        access_key_id: AWS access key ID.
        secret_access_key: AWS secret access key.
        session_token: AWS session token (required for temporary credentials).
        expiration: Unix timestamp when the credentials expire.
        region: AWS region (preserved from the request for convenience).
    """

    access_key_id: str
    secret_access_key: str
    session_token: str
    expiration: float
    region: str

    def to_boto3_kwargs(self) -> Dict[str, str]:
        """Return a dict suitable for unpacking into any boto3 client constructor.

        Example::

            import boto3
            bedrock = boto3.client("bedrock-runtime", **creds.to_boto3_kwargs())
        """
        return {
            "aws_access_key_id": self.access_key_id,
            "aws_secret_access_key": self.secret_access_key,
            "aws_session_token": self.session_token,
            "region_name": self.region,
        }

    @property
    def is_expired(self) -> bool:
        return time.time() >= self.expiration - _REFRESH_BUFFER_SECS


async def oidc_token(
    vm_name: str,
    *,
    audience: str = "sts.amazonaws.com",
    ttl_seconds: int = 300,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> OIDCToken:
    """Request a short-lived OIDC JWT for this sandbox from the CUA OIDC issuer.

    The token is signed by ``https://oidc.cua.ai`` and can be exchanged with
    AWS STS, GCP STS, or Azure AD for short-lived cloud credentials.

    Subject claim format: ``sandbox:workspace:<workspace_slug>:vm:<vm_name>``

    Args:
        vm_name: Name of the sandbox VM. Must belong to the workspace
            associated with the API key.
        audience: Intended audience for the token. Use ``"sts.amazonaws.com"``
            for AWS, ``"https://iam.googleapis.com/..."`` for GCP, etc.
            Defaults to ``"sts.amazonaws.com"``.
        ttl_seconds: Token lifetime in seconds. Max 3600. Default 300 (5 min).
        api_key: Override the global API key.
        base_url: Override the global API base URL.

    Returns:
        ``OIDCToken`` with the raw JWT and expiry metadata.

    Raises:
        httpx.HTTPStatusError: If the API returns a non-2xx status.
        RuntimeError: If no API key is configured.
    """
    key = get_api_key(api_key)
    if not key:
        raise RuntimeError(
            "No CUA API key configured. "
            "Set CUA_API_KEY, call cua_sandbox.configure(api_key=...), "
            "or pass api_key= directly."
        )

    url = (base_url or get_base_url()).rstrip("/") + "/v1/oidc/token"
    issued_at = time.time()

    async with httpx.AsyncClient(timeout=15.0) as client:
        resp = await client.post(
            url,
            headers={
                "Authorization": f"Bearer {key}",
                "Content-Type": "application/json",
            },
            json={
                "vm_name": vm_name,
                "audience": audience,
                "ttl_seconds": ttl_seconds,
            },
        )
        resp.raise_for_status()
        data = resp.json()

    return OIDCToken(
        token=data["token"],
        expires_in=data.get("expires_in", ttl_seconds),
        issued_at=issued_at,
    )


async def aws_credentials(
    role_arn: str,
    *,
    token: Optional[OIDCToken | str] = None,
    vm_name: Optional[str] = None,
    region: str = "us-east-1",
    session_name: str = "cua-sandbox",
    duration_seconds: int = 3600,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> AWSCredentials:
    """Obtain temporary AWS credentials by exchanging a CUA OIDC token with STS.

    Pass either a pre-fetched ``token`` *or* a ``vm_name`` (which will be used
    to fetch a fresh token automatically with audience ``"sts.amazonaws.com"``).

    Requires the ``boto3`` package: ``pip install boto3``.

    Args:
        role_arn: ARN of the IAM role to assume. The role's trust policy must
            allow ``sts:AssumeRoleWithWebIdentity`` from the CUA OIDC provider
            with the correct ``sub`` condition.
        token: Pre-fetched ``OIDCToken`` or raw JWT string. If omitted,
            ``vm_name`` must be provided.
        vm_name: Sandbox name to fetch a fresh token for (if ``token`` not given).
        region: AWS region for the credentials. Default ``"us-east-1"``.
        session_name: RoleSessionName passed to STS. Default ``"cua-sandbox"``.
        duration_seconds: Session duration in seconds (max 3600 for web identity).
        api_key: Override the global API key (used when fetching a fresh token).
        base_url: Override the global API base URL.

    Returns:
        ``AWSCredentials`` with access key, secret, session token, and expiry.

    Raises:
        ValueError: If neither ``token`` nor ``vm_name`` is provided.
        ImportError: If ``boto3`` is not installed.
        botocore.exceptions.ClientError: If STS rejects the token.
    """
    if token is None and vm_name is None:
        raise ValueError("Provide either 'token' or 'vm_name'.")

    # Lazy import — boto3 is optional
    try:
        import boto3
        import botocore.exceptions  # noqa: F401
    except ImportError as e:
        raise ImportError(
            "boto3 is required for aws_credentials(). Install it with: pip install boto3"
        ) from e

    # Fetch token if not provided
    if token is None:
        token = await oidc_token(
            vm_name,  # type: ignore[arg-type]
            audience="sts.amazonaws.com",
            api_key=api_key,
            base_url=base_url,
        )

    raw_token = str(token)

    sts = boto3.client("sts", region_name=region)
    resp = sts.assume_role_with_web_identity(
        RoleArn=role_arn,
        RoleSessionName=session_name,
        WebIdentityToken=raw_token,
        DurationSeconds=duration_seconds,
    )

    creds: Dict[str, Any] = resp["Credentials"]
    expiration = creds["Expiration"].timestamp()

    return AWSCredentials(
        access_key_id=creds["AccessKeyId"],
        secret_access_key=creds["SecretAccessKey"],
        session_token=creds["SessionToken"],
        expiration=expiration,
        region=region,
    )
