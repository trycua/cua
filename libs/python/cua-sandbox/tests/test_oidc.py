"""Unit tests for the OIDC helper module."""

from __future__ import annotations

import time
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from cua_sandbox._oidc import AWSCredentials, OIDCToken, aws_credentials, oidc_token


# ---------------------------------------------------------------------------
# OIDCToken dataclass
# ---------------------------------------------------------------------------


class TestOIDCToken:
    def test_str_returns_jwt(self):
        t = OIDCToken(token="my.jwt.here", expires_in=300, issued_at=time.time())
        assert str(t) == "my.jwt.here"

    def test_expires_at(self):
        now = 1000.0
        t = OIDCToken(token="x", expires_in=300, issued_at=now)
        assert t.expires_at == 1300.0

    def test_is_expired_false_when_fresh(self):
        t = OIDCToken(token="x", expires_in=300, issued_at=time.time())
        assert not t.is_expired

    def test_is_expired_true_when_past_buffer(self):
        # issued 300 + buffer seconds ago → expired
        from cua_sandbox._oidc import _REFRESH_BUFFER_SECS

        t = OIDCToken(token="x", expires_in=300, issued_at=time.time() - 300 - _REFRESH_BUFFER_SECS - 1)
        assert t.is_expired


# ---------------------------------------------------------------------------
# AWSCredentials dataclass
# ---------------------------------------------------------------------------


class TestAWSCredentials:
    def _make(self, expiration: float | None = None) -> AWSCredentials:
        return AWSCredentials(
            access_key_id="AKIA...",
            secret_access_key="secret",
            session_token="tok",
            expiration=expiration or time.time() + 3600,
            region="us-east-1",
        )

    def test_to_boto3_kwargs(self):
        creds = self._make()
        kwargs = creds.to_boto3_kwargs()
        assert kwargs["aws_access_key_id"] == "AKIA..."
        assert kwargs["aws_secret_access_key"] == "secret"
        assert kwargs["aws_session_token"] == "tok"
        assert kwargs["region_name"] == "us-east-1"

    def test_is_expired_false_when_fresh(self):
        creds = self._make(expiration=time.time() + 3600)
        assert not creds.is_expired

    def test_is_expired_true_when_stale(self):
        from cua_sandbox._oidc import _REFRESH_BUFFER_SECS

        creds = self._make(expiration=time.time() - _REFRESH_BUFFER_SECS - 1)
        assert creds.is_expired


# ---------------------------------------------------------------------------
# oidc_token()
# ---------------------------------------------------------------------------


class TestOidcToken:
    @pytest.mark.asyncio
    async def test_returns_oidc_token(self):
        mock_response = MagicMock()
        mock_response.raise_for_status = MagicMock()
        mock_response.json.return_value = {
            "token": "header.payload.sig",
            "expires_in": 300,
            "token_type": "Bearer",
        }

        mock_client = AsyncMock()
        mock_client.__aenter__ = AsyncMock(return_value=mock_client)
        mock_client.__aexit__ = AsyncMock(return_value=None)
        mock_client.post = AsyncMock(return_value=mock_response)

        with patch("cua_sandbox._oidc.httpx.AsyncClient", return_value=mock_client):
            with patch("cua_sandbox._oidc.get_api_key", return_value="sk-test"):
                token = await oidc_token("my-agent", audience="sts.amazonaws.com")

        assert isinstance(token, OIDCToken)
        assert token.token == "header.payload.sig"
        assert token.expires_in == 300
        assert not token.is_expired

        # Verify the request body
        call_kwargs = mock_client.post.call_args
        assert call_kwargs.kwargs["json"]["vm_name"] == "my-agent"
        assert call_kwargs.kwargs["json"]["audience"] == "sts.amazonaws.com"

    @pytest.mark.asyncio
    async def test_raises_when_no_api_key(self):
        with patch("cua_sandbox._oidc.get_api_key", return_value=None):
            with pytest.raises(RuntimeError, match="No CUA API key"):
                await oidc_token("my-agent")

    @pytest.mark.asyncio
    async def test_uses_base_url(self):
        mock_response = MagicMock()
        mock_response.raise_for_status = MagicMock()
        mock_response.json.return_value = {"token": "t", "expires_in": 300}

        mock_client = AsyncMock()
        mock_client.__aenter__ = AsyncMock(return_value=mock_client)
        mock_client.__aexit__ = AsyncMock(return_value=None)
        mock_client.post = AsyncMock(return_value=mock_response)

        with patch("cua_sandbox._oidc.httpx.AsyncClient", return_value=mock_client):
            with patch("cua_sandbox._oidc.get_api_key", return_value="sk-test"):
                await oidc_token("vm", base_url="https://staging.api.cua.ai")

        url = mock_client.post.call_args.args[0]
        assert url == "https://staging.api.cua.ai/v1/oidc/token"


# ---------------------------------------------------------------------------
# aws_credentials()
# ---------------------------------------------------------------------------


class TestAwsCredentials:
    def _mock_sts_response(self) -> dict:
        import datetime

        expiry = datetime.datetime(2099, 1, 1, tzinfo=datetime.timezone.utc)
        return {
            "Credentials": {
                "AccessKeyId": "ASIA...",
                "SecretAccessKey": "supersecret",
                "SessionToken": "session-tok",
                "Expiration": expiry,
            }
        }

    @pytest.mark.asyncio
    async def test_uses_provided_token(self):
        token = OIDCToken(token="raw.jwt", expires_in=300, issued_at=time.time())
        mock_sts = MagicMock()
        mock_sts.assume_role_with_web_identity.return_value = self._mock_sts_response()

        with patch("cua_sandbox._oidc.boto3") as mock_boto3:
            mock_boto3.client.return_value = mock_sts
            creds = await aws_credentials(
                "arn:aws:iam::123:role/test",
                token=token,
                region="eu-west-1",
            )

        mock_sts.assume_role_with_web_identity.assert_called_once()
        call_kwargs = mock_sts.assume_role_with_web_identity.call_args.kwargs
        assert call_kwargs["WebIdentityToken"] == "raw.jwt"
        assert call_kwargs["RoleArn"] == "arn:aws:iam::123:role/test"

        assert creds.access_key_id == "ASIA..."
        assert creds.region == "eu-west-1"
        assert not creds.is_expired

    @pytest.mark.asyncio
    async def test_accepts_raw_string_token(self):
        mock_sts = MagicMock()
        mock_sts.assume_role_with_web_identity.return_value = self._mock_sts_response()

        with patch("cua_sandbox._oidc.boto3") as mock_boto3:
            mock_boto3.client.return_value = mock_sts
            creds = await aws_credentials(
                "arn:aws:iam::123:role/test",
                token="raw.string.jwt",
            )

        call_kwargs = mock_sts.assume_role_with_web_identity.call_args.kwargs
        assert call_kwargs["WebIdentityToken"] == "raw.string.jwt"
        assert isinstance(creds, AWSCredentials)

    @pytest.mark.asyncio
    async def test_fetches_token_when_vm_name_given(self):
        mock_sts = MagicMock()
        mock_sts.assume_role_with_web_identity.return_value = self._mock_sts_response()

        fetched_token = OIDCToken(token="fetched.jwt", expires_in=300, issued_at=time.time())
        with patch("cua_sandbox._oidc.oidc_token", AsyncMock(return_value=fetched_token)):
            with patch("cua_sandbox._oidc.boto3") as mock_boto3:
                mock_boto3.client.return_value = mock_sts
                await aws_credentials("arn:aws:iam::123:role/test", vm_name="my-vm")

        call_kwargs = mock_sts.assume_role_with_web_identity.call_args.kwargs
        assert call_kwargs["WebIdentityToken"] == "fetched.jwt"

    @pytest.mark.asyncio
    async def test_raises_without_token_or_vm_name(self):
        with pytest.raises(ValueError, match="Provide either"):
            await aws_credentials("arn:aws:iam::123:role/test")

    @pytest.mark.asyncio
    async def test_raises_without_boto3(self):
        token = OIDCToken(token="t", expires_in=300, issued_at=time.time())
        with patch.dict("sys.modules", {"boto3": None, "botocore": None, "botocore.exceptions": None}):
            with pytest.raises(ImportError, match="boto3"):
                await aws_credentials("arn:aws:iam::123:role/test", token=token)
