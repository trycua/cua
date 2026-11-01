"""Connection config, the `E2B` client, signed URLs, sandbox option helpers and the
pure API helpers: behaviour and error texts of e2b 2.58 (read from its source), over
the in-memory backend where a sandbox is involved. No network."""

from __future__ import annotations

import asyncio
import gzip
import io
import logging
import types
from datetime import datetime, timezone
from unittest import mock

import e2b
import pytest
from e2b import (
    ApiClient,
    AsyncSandbox,
    AsyncSecret,
    AuthenticationException,
    ConnectionConfig,
    InvalidArgumentException,
    RateLimitException,
    Sandbox,
    SandboxException,
    Secret,
    ServiceBusyException,
    get_signature,
)
from e2b._backend import set_backend
from e2b.api import (
    ProxyConfig,
    api_exception_from_code,
    encode_path_param,
    handle_api_exception,
    make_logging_event_hooks,
    proxy_to_config,
    validate_api_key,
)
from e2b.connection_config import (
    ApiParams,
    ClientFactory,
    default_username,
    merge_api_params,
    resolve_http_version,
)
from e2b.exceptions import (
    SandboxNotRunningException,
    SandboxUnreachableException,
    TimeoutException,
    VolumeNotFoundException,
    VolumePathNotFoundException,
    format_sandbox_unavailable_exception,
    is_sandbox_not_found_message,
    is_sandbox_port_not_open_message,
)
from e2b.io_utils import agzip_iter, aiter_io_chunks, gzip_iter, iter_io_chunks
from e2b.paginator import PaginatorBase
from e2b.retry import is_replayable, parse_retry_after, resolve_max_retries
from e2b.sandbox.iam import (
    IamTokenPlaceholders,
    iam_token_placeholder,
    validate_iam_token_name,
)
from e2b.sandbox.main import SandboxBase
from e2b.sandbox.sandbox_api import (
    SandboxInfo,
    SandboxQuery,
    build_iam_config,
    build_lifecycle_config,
    build_network_config,
    build_network_update_body,
    from_client_lifecycle,
    from_client_network_config,
    resolve_connect_memory,
    resolve_snapshot_memory,
    to_sandbox_create_response,
    validate_fork_count,
)
from e2b.sandbox_domains import is_supported_sandbox_domain
from fake_backend import FakeBackend

ENV = [
    "E2B_DOMAIN",
    "E2B_DEBUG",
    "E2B_API_KEY",
    "E2B_API_URL",
    "E2B_SANDBOX_URL",
    "E2B_HTTP_VERSION",
]


@pytest.fixture(autouse=True)
def clean_env(monkeypatch):
    for name in ENV:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(ConnectionConfig, "_integration", None)


@pytest.fixture
def backend(monkeypatch):
    monkeypatch.delenv("CUA_E2B_TEMPLATES", raising=False)
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


# -- ConnectionConfig ---------------------------------------------------------------


def test_defaults_without_environment():
    c = ConnectionConfig()
    assert (c.domain, c.debug, c.api_key) == ("e2b.app", False, None)
    assert c.api_url == "https://api.e2b.app"
    assert c.request_timeout == 60.0
    assert (c.retries, c.http_version) == (3, "2")
    assert c.envd_port == 49983 and c.proxy is None and c.validate_api_key is None
    assert c.headers["User-Agent"].startswith("e2b-python-sdk/")


def test_environment_variables_are_read_as_e2b_reads_them(monkeypatch):
    monkeypatch.setenv("E2B_DOMAIN", "example.dev")
    monkeypatch.setenv("E2B_API_KEY", "e2b_abc")
    monkeypatch.setenv("E2B_HTTP_VERSION", "1.1")
    c = ConnectionConfig()
    assert (c.domain, c.api_key, c.http_version) == ("example.dev", "e2b_abc", "1.1")
    assert c.api_url == "https://api.example.dev"
    monkeypatch.setenv("E2B_API_URL", "http://api.local:9")
    assert ConnectionConfig().api_url == "http://api.local:9"
    # An explicit value beats the environment.
    c = ConnectionConfig(domain="x.io", api_key="k", http_version="2", api_url="http://z")
    assert (c.domain, c.api_key, c.http_version, c.api_url) == ("x.io", "k", "2", "http://z")


@pytest.mark.parametrize(
    "value,expected", [("true", True), ("TRUE", True), ("false", False), ("1", False)]
)
def test_debug_is_true_only_for_the_word_true(monkeypatch, value, expected):
    monkeypatch.setenv("E2B_DEBUG", value)
    assert ConnectionConfig().debug is expected


def test_debug_mode_points_at_localhost():
    c = ConnectionConfig(debug=True)
    assert c.api_url == "http://localhost:3000"
    assert c.get_host("sb", "e2b.app", 8080) == "localhost:8080"
    assert c.get_sandbox_url("sb", "e2b.app") == "http://localhost:49983"
    assert c.get_sandbox_direct_url("sb", "e2b.app", 5) == "http://localhost:5"


def test_sandbox_urls_by_domain():
    c = ConnectionConfig()
    assert c.get_host("sbid", "e2b.app", 3000) == "3000-sbid.e2b.app"
    # Supported domains share one sandbox endpoint; others use the per-sandbox host.
    assert c.get_sandbox_url("sbid", "e2b.app") == "https://sandbox.e2b.app"
    assert c.get_sandbox_url("sbid", "corp.example") == "https://49983-sbid.corp.example"
    assert c.get_sandbox_url("sbid", "") == "https://sandbox.e2b.app"
    assert c.get_sandbox_direct_url("sbid", "e2b.app") == "https://49983-sbid.e2b.app"
    assert c.get_sandbox_direct_url("sbid", "e2b.app", 22) == "https://22-sbid.e2b.app"
    assert is_supported_sandbox_domain("e2b.dev") and not is_supported_sandbox_domain("x.dev")


def test_sandbox_url_override_wins(monkeypatch):
    monkeypatch.setenv("E2B_SANDBOX_URL", "http://envd.test:1")
    c = ConnectionConfig()
    assert c.get_sandbox_url("a", "e2b.app") == "http://envd.test:1"
    assert c.get_sandbox_direct_url("a", "e2b.app") == "http://envd.test:1"
    assert ConnectionConfig(sandbox_url="http://o:2").get_sandbox_url("a", "d") == "http://o:2"


@pytest.mark.parametrize("bad", [-1, 1.5, "3", True, None.__class__])
def test_invalid_retries_raise_with_e2bs_text(bad):
    with pytest.raises(InvalidArgumentException) as info:
        ConnectionConfig(retries=bad)
    assert str(info.value) == f"Invalid retries={bad!r}: expected a non-negative integer."


def test_retries_zero_is_valid():
    assert ConnectionConfig(retries=0).retries == 0


def test_invalid_http_version_texts(monkeypatch):
    with pytest.raises(InvalidArgumentException) as info:
        ConnectionConfig(http_version="3")
    assert str(info.value) == "http_version must be '1.1' or '2', got '3'"
    monkeypatch.setenv("E2B_HTTP_VERSION", "h2")
    with pytest.raises(InvalidArgumentException) as info:
        ConnectionConfig()
    assert str(info.value) == "E2B_HTTP_VERSION must be '1.1' or '2', got 'h2'"
    assert resolve_http_version("1.1") == "1.1"


def test_request_timeout_rules():
    assert ConnectionConfig(request_timeout=5).request_timeout == 5
    assert ConnectionConfig(request_timeout=0).request_timeout is None
    c = ConnectionConfig(request_timeout=5)
    assert c.get_request_timeout() == 5
    assert c.get_request_timeout(0) is None
    assert c.get_request_timeout(9) == 9
    assert ConnectionConfig(request_timeout=0).get_request_timeout() is None


def test_headers_and_user_agent():
    c = ConnectionConfig(headers={"a": "1"}, api_headers={"b": "2", "a": "3"})
    assert c.headers["a"] == "3" and c.headers["b"] == "2"
    mine = ConnectionConfig(headers={"User-Agent": "mine"})
    assert mine.headers["User-Agent"] == "mine"
    assert mine.sandbox_headers == {"User-Agent": "mine"}
    c = ConnectionConfig(extra_sandbox_headers={"X-Y": "z"})
    assert c.sandbox_headers == {"User-Agent": c.headers["User-Agent"], "X-Y": "z"}


def test_set_integration_tags_new_configs_only():
    before = ConnectionConfig()
    ConnectionConfig.set_integration("e2b-code-interpreter/0.1.0")
    after = ConnectionConfig()
    assert after.headers["User-Agent"].endswith(" e2b-code-interpreter/0.1.0")
    assert "e2b-code-interpreter" not in before.headers["User-Agent"]
    # A config built before keeps following the integration through get_api_params.
    assert before.get_api_params()["headers"]["User-Agent"].endswith("e2b-code-interpreter/0.1.0")
    ConnectionConfig.set_integration(None)
    assert "e2b-code-interpreter" not in ConnectionConfig().headers["User-Agent"]


def test_get_api_params_merges_per_call_options():
    log = logging.getLogger("t")
    c = ConnectionConfig(api_key="k", domain="d.io", request_timeout=7, logger=log, retries=1)
    p = c.get_api_params(request_timeout=3, headers={"h": "1"}, api_headers={"h": "2"}, retries=5)
    assert p["api_key"] == "k" and p["domain"] == "d.io" and p["logger"] is log
    assert p["request_timeout"] == 3 and p["retries"] == 5
    assert p["headers"]["h"] == "2"
    assert p["api_url"] == c.api_url and p["http_version"] == "2" and p["debug"] is False
    # It is a copy: the config's own headers are untouched.
    assert "h" not in c.headers
    # The result can build an equivalent config.
    again = ConnectionConfig(**{k: v for k, v in p.items() if k != "api_headers"})
    assert again.api_key == "k" and again.retries == 5


def test_merge_api_params_rules():
    assert merge_api_params(None, {"a": 1}) == {"a": 1}
    assert merge_api_params({}, {"a": 1}) == {"a": 1}
    bound = {"api_key": "bound", "domain": "b.io"}
    assert merge_api_params(bound, {"api_key": "call"}) == {"api_key": "call", "domain": "b.io"}
    # A per-call None falls back to the bound value instead of clearing it.
    assert merge_api_params(bound, {"api_key": None}) == bound
    assert bound == {"api_key": "bound", "domain": "b.io"}


def test_default_username_and_types():
    assert default_username == "user"
    assert e2b.Username is str and e2b.HttpVersion and ApiParams.__total__ is False


# -- the E2B client -----------------------------------------------------------------


def test_client_binds_params_to_subclasses():
    client = e2b.E2B(api_key="e2b_one", domain="one.dev", request_timeout=9)
    for name, base in [
        ("Sandbox", Sandbox),
        ("AsyncSandbox", AsyncSandbox),
        ("Volume", e2b.Volume),
        ("AsyncVolume", e2b.AsyncVolume),
        ("Template", e2b.Template),
        ("AsyncTemplate", e2b.AsyncTemplate),
        ("Secret", Secret),
        ("AsyncSecret", AsyncSecret),
    ]:
        cls = getattr(client, name)
        assert issubclass(cls, base) and cls is not base and cls.__name__ == name
        assert cls._bound_api_params == {
            "api_key": "e2b_one",
            "domain": "one.dev",
            "request_timeout": 9,
        }
    assert Secret._bound_api_params == {}  # the top-level classes are untouched


def test_clients_are_isolated_and_copy_their_params():
    opts = {"api_key": "a"}
    one = e2b.E2B(**opts)
    two = e2b.E2B(api_key="b")
    opts["api_key"] = "changed"
    assert one.Secret._bound_api_params == {"api_key": "a"}
    assert two.Secret._bound_api_params == {"api_key": "b"}
    assert one.Secret is not two.Secret


def test_resource_classes_resolve_bound_params_under_per_call_params():
    client = e2b.E2B(api_key="bound", domain="b.io")
    assert client.Secret._resolve_api_params(api_key="call") == {
        "api_key": "call",
        "domain": "b.io",
    }
    assert client.Secret._resolve_api_params(domain=None) == {"api_key": "bound", "domain": "b.io"}
    assert issubclass(Secret, ClientFactory)


async def test_bound_params_are_validated_by_the_calls_that_use_them():
    client = e2b.E2B(retries=-2)
    with pytest.raises(InvalidArgumentException) as info:
        await client.AsyncSecret.get_info("x")
    assert str(info.value) == "Invalid retries=-2: expected a non-negative integer."


def test_client_params_type_has_the_api_param_keys():
    assert set(e2b.E2BClientParams.__annotations__) == set(ApiParams.__annotations__)


# -- signatures and file URLs ---------------------------------------------------------

# Vectors computed with e2b 2.58.0's get_signature at time.time() == 1_700_000_000.9.
VECTORS = [
    (
        ("/home/user/a.txt", "read", "user", "tok-123", None),
        "v1_WU421hL9oufM/6QQ4tu2j5JE6pCOVNqn8RZYrco28l8",
        None,
    ),
    (
        ("/home/user/a.txt", "write", "user", "tok-123", None),
        "v1_LO7Oe4sEMdDgq6v9ukLJHh26wkvxjR8o0gdA3kPuo/4",
        None,
    ),
    (
        ("/tmp/x y", "read", None, "secret", 60),
        "v1_HH/5XosZBUFuEI4sDIppJ2y/c1Gc1C8kmV/pXiXh2Z8",
        1700000060,
    ),
    (
        ("/", "write", "root", "t", 3600),
        "v1_hqHWnFyh+Bs0Yz4UhvkzeYUfzoedrzotqGf+JDVFjO8",
        1700003600,
    ),
]


@pytest.mark.parametrize("args,signature,expiration", VECTORS)
def test_get_signature_matches_e2b(args, signature, expiration):
    with mock.patch("time.time", return_value=1_700_000_000.9):
        assert get_signature(*args) == {"signature": signature, "expiration": expiration}


def test_get_signature_needs_a_token():
    for token in (None, ""):
        with pytest.raises(
            ValueError, match="Access token is not set and signature cannot be generated!"
        ):
            get_signature("/a", "read", "user", token)


def _base(version="0.5.7", token="tok", domain="e2b.app", **cfg):
    return SandboxBase(
        "sbid", version, token, domain, ConnectionConfig(api_key="k", **cfg), "traffic"
    )


def test_sandbox_base_identity_and_hosts():
    sb = _base()
    assert (sb.sandbox_id, sb.sandbox_domain, sb.traffic_access_token) == (
        "sbid",
        "e2b.app",
        "traffic",
    )
    assert sb.envd_api_url == "https://sandbox.e2b.app"
    assert sb.envd_direct_url == "https://49983-sbid.e2b.app"
    assert sb.get_host(3000) == "3000-sbid.e2b.app"
    assert sb.get_mcp_url() == "https://50005-sbid.e2b.app/mcp"
    assert (SandboxBase.default_template, SandboxBase.default_mcp_template) == (
        "base",
        "mcp-gateway",
    )
    assert (SandboxBase.default_sandbox_timeout, SandboxBase.mcp_port) == (300, 50005)
    # The domain falls back to the connection config's.
    assert (
        SandboxBase("i", "0.5.7", None, None, ConnectionConfig(domain="c.io")).sandbox_domain
        == "c.io"
    )


def test_download_and_upload_urls_match_e2b():
    sb = _base()
    assert sb.download_url("/a b.txt") == (
        "https://49983-sbid.e2b.app/files?path=%2Fa%20b.txt"
        "&signature=v1_a4OGYehSzO3jBuMXDgXaf64WGjQj84jfvvQyWyrQ8uQ"
    )
    with mock.patch("time.time", return_value=1_700_000_000.9):
        assert sb.download_url("/a b.txt", user="user", use_signature_expiration=60) == (
            "https://49983-sbid.e2b.app/files?path=%2Fa%20b.txt&username=user"
            "&signature=v1_u%2BzFlwuxE3MVZUYkVYOqZoik6u6tf88x9qZPmz4FJQA&signature_expiration=1700000060"
        )
    assert sb.upload_url("/data/f.bin") == (
        "https://49983-sbid.e2b.app/files?path=%2Fdata%2Ff.bin"
        "&signature=v1_%2FlF%2FPw%2F04PR3%2BxsblfhwjxP6qlxSQw%2FicVlDWQw25Q4"
    )


def test_unsecured_sandbox_urls_carry_no_signature():
    old = _base(version="0.3.0", token=None, domain="example.com")
    # envd before 0.4.0 needs the user named; the URL is unsigned without a token.
    assert old.upload_url("/x") == "https://49983-sbid.example.com/files?path=%2Fx&username=user"
    new = _base(version="0.5.7", token=None)
    assert new.download_url("/x") == "https://49983-sbid.e2b.app/files?path=%2Fx"
    assert new.download_url("/x", user="root").endswith("&username=root")
    # cua-e2b's own envd version string counts as a current envd.
    assert "username" not in _base(version="cua-spacesd", token=None).download_url("/x")


def test_signature_expiration_needs_a_secured_sandbox():
    for call in (_base(token=None).download_url, _base(token=None).upload_url):
        with pytest.raises(InvalidArgumentException) as info:
            call("/x", use_signature_expiration=10)
        assert (
            str(info.value)
            == "Signature expiration can be used only when sandbox is created as secured."
        )


# -- exceptions -----------------------------------------------------------------------


def test_new_exception_classes_have_e2bs_bases():
    assert issubclass(VolumeNotFoundException, e2b.NotFoundException)
    assert issubclass(VolumePathNotFoundException, e2b.NotFoundException)
    assert e2b.SecretException.__mro__[1] is Exception
    assert issubclass(e2b.SecretNotFoundException, e2b.SecretException)
    assert not issubclass(e2b.SecretException, SandboxException)
    # The volume module re-exports the same classes.
    assert e2b.volume.VolumeNotFoundException is VolumeNotFoundException


def test_status_codes_of_exceptions():
    assert SandboxException("x").status_code is None
    assert SandboxException("x", status_code=418).status_code == 418
    assert RateLimitException("slow").status_code == 429
    assert ServiceBusyException.status_code == 503


def test_unavailable_message_mapping():
    assert is_sandbox_not_found_message("The sandbox was not found")
    assert is_sandbox_port_not_open_message("the port is not open")
    gone = format_sandbox_unavailable_exception("The sandbox was not found")
    assert isinstance(gone, SandboxNotRunningException)
    assert str(gone).startswith(
        "The sandbox was not found: The sandbox was killed or reached its timeout."
    )
    closed = format_sandbox_unavailable_exception("port is not open")
    assert type(closed) is SandboxUnreachableException
    assert "envd inside it could not be reached" in str(closed)
    other = format_sandbox_unavailable_exception("stream ended")
    assert type(other) is SandboxUnreachableException and "its state is unknown" in str(other)
    assert isinstance(other, TimeoutException)


# -- api helpers ----------------------------------------------------------------------


def test_encode_path_param_keeps_one_segment():
    assert encode_path_param("team/name:tag") == "team%2Fname%3Atag"
    assert encode_path_param("plain-id_1") == "plain-id_1"


def test_api_exception_from_code_messages():
    e = api_exception_from_code(401)
    assert type(e) is AuthenticationException
    assert str(e) == "401: Unauthorized, please check your credentials."
    assert str(api_exception_from_code(401, "bad key")).endswith(" - bad key")
    e = api_exception_from_code(429, "slow")
    assert type(e) is RateLimitException and e.status_code == 429
    assert str(e) == "429: Rate limit exceeded, please try again later. - slow"
    e = api_exception_from_code(503)
    assert type(e) is ServiceBusyException
    assert str(e) == "503: Service temporarily unavailable, please retry."
    e = api_exception_from_code(404, "nope")
    assert type(e) is SandboxException and str(e) == "404: nope" and e.status_code == 404
    assert type(api_exception_from_code(500, "x", e2b.TemplateException)) is e2b.TemplateException


def test_handle_api_exception_reads_the_json_message():
    def resp(code, content):
        return types.SimpleNamespace(status_code=code, content=content)

    e = handle_api_exception(resp(400, '{"message": "bad things"}'))
    assert str(e) == "400: bad things" and e.status_code == 400
    # No JSON message: the raw body is the text.
    assert str(handle_api_exception(resp(502, b"gateway"))) == "502: b'gateway'"
    assert str(handle_api_exception(resp(500, "not json"))) == "500: not json"
    assert type(handle_api_exception(resp(401, ""))) is AuthenticationException
    assert type(handle_api_exception(resp(503, ""))) is ServiceBusyException
    assert type(handle_api_exception(resp(429, ""))) is RateLimitException


def test_proxy_to_config():
    assert proxy_to_config(None) is None
    assert proxy_to_config("http://u:p@h:8") == ProxyConfig("http://u:p@h:8")
    assert hash(proxy_to_config("http://h:1")) == hash(ProxyConfig("http://h:1"))
    with pytest.raises(
        InvalidArgumentException, match="Only URL-string, httpx.URL, and httpx.Proxy"
    ):
        proxy_to_config(42)


def test_proxy_to_config_with_httpx_objects():
    httpx = pytest.importorskip("httpx")
    assert proxy_to_config(httpx.URL("socks5://h:1")) == ProxyConfig("socks5://h:1")
    cfg = proxy_to_config(httpx.Proxy("http://h:2", headers={"X": "y"}))
    assert cfg is not None and cfg.url.startswith("http://h:2") and cfg.headers == (("x", "y"),)


def test_logging_event_hooks():
    assert make_logging_event_hooks(None) == {}
    log = logging.getLogger("hooks")
    hooks = make_logging_event_hooks(log)
    with mock.patch.object(log, "info") as info, mock.patch.object(log, "error") as error:
        hooks["request"][0](types.SimpleNamespace(method="GET", url="http://x"))
        hooks["response"][0](types.SimpleNamespace(status_code=200))
        hooks["response"][0](types.SimpleNamespace(status_code=404))
    assert [c.args[0] for c in info.call_args_list] == ["Request GET http://x", "Response 200"]
    assert [c.args[0] for c in error.call_args_list] == ["Response 404"]


def test_validate_api_key_2_42():
    validate_api_key("e2b_" + "0f" * 20)
    with pytest.raises(AuthenticationException, match="Invalid API key format"):
        validate_api_key("nope")


def test_api_client_is_a_data_holder_without_a_rest_plane():
    with pytest.raises(AuthenticationException, match="API key is required"):
        ApiClient(ConnectionConfig())
    config = ConnectionConfig(api_key="e2b_x", request_timeout=4, headers={"H": "1"})
    client = ApiClient(config)
    assert client.token == "e2b_x" and client._base_url == "https://api.e2b.app"
    assert client.auth_header_name == "X-API-KEY" and client.prefix == ""
    assert client._headers["H"] == "1" and client._headers["publisher"] == "cua-e2b"
    more = client.with_headers({"Z": "2"}).with_timeout(9)
    assert more._headers["Z"] == "2" and "Z" not in client._headers and more._timeout == 9
    assert more.with_cookies({"c": "d"})._cookies == {"c": "d"}
    for getter in (client.get_httpx_client, client.get_async_httpx_client):
        with pytest.raises(SandboxException, match="no E2B REST control plane"):
            getter()
    sentinel = object()
    assert client.set_httpx_client(sentinel).get_httpx_client() is sentinel
    assert type(e2b.api.AsyncApiClient(config)).__name__ == "AsyncApiClient"


# -- retry, io, paginator ---------------------------------------------------------------


def test_resolve_max_retries_and_retry_after():
    assert resolve_max_retries(0) == 0 and resolve_max_retries(7) == 7
    for bad in (-1, True, 2.0, "1"):
        with pytest.raises(InvalidArgumentException):
            resolve_max_retries(bad)
    assert parse_retry_after(" 12 ") == 12
    for value in (
        None,
        "",
        "-1",
        "1.5",
        "Wed, 21 Oct 2026 07:28:00 GMT",
        "99999999999",
        "2147483648",
    ):
        assert parse_retry_after(value) is None
    assert parse_retry_after("2147483647") == 2_147_483_647


def test_is_replayable_rules():
    def req(method, path):
        return types.SimpleNamespace(method=method, url=types.SimpleNamespace(path=path))

    assert is_replayable(req("GET", "/sandboxes"))
    assert is_replayable(req("DELETE", "/sandboxes/abc"))
    assert is_replayable(req("POST", "/sandboxes/abc/pause"))
    assert is_replayable(req("POST", "/v2/sandboxes/abc/connect"))
    assert not is_replayable(req("POST", "/sandboxes"))
    assert not is_replayable(req("POST", "/secrets"))


def test_io_chunking_and_gzip():
    data = b"x" * 70_000 + b"tail"
    assert b"".join(iter_io_chunks(io.BytesIO(data))) == data
    assert [len(c) for c in iter_io_chunks(io.BytesIO(data))] == [65_536, 4468]
    assert b"".join(iter_io_chunks(io.StringIO("héllo"))) == "héllo".encode()
    assert gzip.decompress(b"".join(gzip_iter(iter([b"ab", b"cd"])))) == b"abcd"

    async def run():
        chunks = [c async for c in aiter_io_chunks(io.BytesIO(data))]

        async def source():
            for c in chunks:
                yield c

        packed = b"".join([c async for c in agzip_iter(source())])
        return b"".join(chunks), gzip.decompress(packed)

    raw, unpacked = asyncio.run(run())
    assert raw == data and unpacked == data


def test_paginator_base_state():
    p = PaginatorBase(limit=5, next_token="t0", api_key="k")
    assert (p.limit, p.has_next, p.next_token, p._opts) == (5, True, "t0", {"api_key": "k"})
    p._update_pagination({"x-next-token": "t1"})
    assert (p.has_next, p.next_token) == (True, "t1")
    p._update_pagination({})
    assert (p.has_next, p.next_token) == (False, None)


# -- iam --------------------------------------------------------------------------------


@pytest.mark.parametrize("name", ["", "a{b", "a}b", "a\x00b", "a\x9fb", 5])
def test_invalid_iam_token_names(name):
    with pytest.raises(InvalidArgumentException) as info:
        validate_iam_token_name(name)
    assert str(info.value).startswith(
        f"iam token name {name!r} is not usable: a token name must be a non-empty string"
    )


def test_iam_placeholders():
    assert iam_token_placeholder("aws") == "${e2b.identity.tokens.aws}"
    registered = IamTokenPlaceholders(["aws", "gcp", "aws"], validate=True)
    assert list(registered) == ["aws", "gcp"] and len(registered) == 2
    assert registered["gcp"] == "${e2b.identity.tokens.gcp}"
    assert "aws" in registered and "nope" not in registered
    with pytest.raises(InvalidArgumentException) as info:
        registered["nope"]
    assert str(info.value) == (
        "Network transform references iam token 'nope', which is not registered. "
        "Registered tokens: 'aws', 'gcp'."
    )
    with pytest.raises(InvalidArgumentException, match="Pass it to Sandbox.create as iam="):
        IamTokenPlaceholders([], validate=True)["x"]
    assert IamTokenPlaceholders([], validate=False)["any"] == "${e2b.identity.tokens.any}"


# -- sandbox option helpers ---------------------------------------------------------------


def test_validate_fork_count():
    validate_fork_count(None)
    validate_fork_count(1)
    validate_fork_count(20)
    for bad in (0, 21, True, 2.5, "3"):
        with pytest.raises(InvalidArgumentException) as info:
            validate_fork_count(bad)
        assert str(info.value) == "count must be an integer between 1 and 20"


def test_snapshot_and_connect_memory():
    unset = resolve_snapshot_memory("mode", None)
    assert not unset and unset is e2b.api.client.UNSET
    assert resolve_snapshot_memory("mode", "full") is True
    assert resolve_snapshot_memory("mode", "filesystem") is False
    assert resolve_snapshot_memory("mode", None, False) is False
    with pytest.raises(InvalidArgumentException) as info:
        resolve_snapshot_memory("mode", "full", True)
    assert str(info.value) == "Pass either mode or the deprecated keep_memory, not both."
    with pytest.raises(InvalidArgumentException) as info:
        resolve_snapshot_memory("mode", "half")
    assert str(info.value) == "mode must be one of: full, filesystem (got 'half')."
    assert resolve_connect_memory(None) is e2b.api.client.UNSET
    assert resolve_connect_memory("restore") is e2b.api.client.UNSET
    assert resolve_connect_memory("reboot") is False
    with pytest.raises(InvalidArgumentException) as info:
        resolve_connect_memory("nap")
    assert str(info.value) == "on_resume must be one of: restore, reboot (got 'nap')."


def test_build_lifecycle_config():
    empty = build_lifecycle_config(None)
    assert not empty.auto_pause and not empty.auto_pause_memory and not empty.auto_resume
    kill = build_lifecycle_config({"on_timeout": "kill"})
    assert kill.auto_pause is False and not kill.auto_pause_memory
    pause = build_lifecycle_config({"on_timeout": "pause", "auto_resume": True})
    assert pause.auto_pause is True and pause.auto_resume == {"enabled": True}
    fs = build_lifecycle_config({"on_timeout": {"action": "pause", "mode": "filesystem"}})
    assert fs.auto_pause is True and fs.auto_pause_memory is False
    off = build_lifecycle_config({"on_timeout": "pause", "auto_resume": False})
    assert off.auto_resume == {"enabled": False}


@pytest.mark.parametrize(
    "lifecycle,text",
    [
        ({"on_timeout": "nap"}, "on_timeout must be one of: pause, kill (got 'nap')."),
        (
            {"on_timeout": {"action": "nap"}},
            "on_timeout[\"action\"] must be one of: pause, kill (got 'nap').",
        ),
        (
            {"on_timeout": {"action": "kill", "mode": "full"}},
            "mode is only allowed when on_timeout action is 'pause'.",
        ),
        (
            {"on_timeout": "kill", "auto_resume": True},
            "auto_resume can only be True when on_timeout action is 'pause'.",
        ),
        (
            {"on_timeout": {"action": "pause", "mode": "filesystem"}, "auto_resume": True},
            "auto_resume: True is not a valid value with mode: 'filesystem' - a filesystem-only "
            "snapshot cannot be auto-resumed by traffic and must be resumed explicitly using "
            "Sandbox.connect().",
        ),
        (
            {"on_timeout": {"action": "pause", "mode": "full", "keep_memory": True}},
            'Pass either on_timeout["mode"] or the deprecated keep_memory, not both.',
        ),
    ],
)
def test_build_lifecycle_config_errors(lifecycle, text):
    with pytest.raises(InvalidArgumentException) as info:
        build_lifecycle_config(lifecycle)
    assert str(info.value) == text


def test_build_iam_config():
    assert build_iam_config(None) is None and build_iam_config({}) is None
    assert build_iam_config({"tokens": {}}) is None
    token = Secret.iam_token(audience="sts.amazonaws.com", token_type="JWT-SVID")
    assert token == {"audience": "sts.amazonaws.com", "token_type": "JWT-SVID"}
    assert build_iam_config({"tokens": {"aws": token}}) == {"tokens": {"aws": token}}
    with pytest.raises(InvalidArgumentException) as info:
        build_iam_config({"tokens": {"aws": {"audience": "a", "tokenType": "JWT-SVID"}}})
    assert str(info.value) == (
        "iam token 'aws' must be a dict with string 'audience' and 'token_type' values "
        "(snake_case 'token_type', not 'tokenType')."
    )
    with pytest.raises(InvalidArgumentException, match="iam token name 'a}b' is not usable"):
        build_iam_config({"tokens": {"a}b": token}})


def test_build_network_config_egress_rules():
    assert build_network_config(None) is None
    iam = build_iam_config({"tokens": {"aws": {"audience": "a", "token_type": "JWT-SVID"}}})
    body = build_network_config(
        {
            "allow_out": lambda ctx: list(ctx.rules),
            "deny_out": lambda ctx: [ctx.all_traffic],
            "rules": {
                "api.example.com": [
                    {"transform": {"headers": {"X-A": "1"}}},
                    {
                        "transform": lambda ctx: {
                            "headers": {"Authorization": f"Bearer {ctx.iam.tokens['aws']}"}
                        }
                    },
                    {},
                ]
            },
            "allow_public_traffic": False,
            "mask_request_host": "${PORT}-app.example.com",
            "https_ports": [3000],
        },
        iam,
    )
    assert body == {
        "allow_out": ["api.example.com"],
        "deny_out": ["0.0.0.0/0"],
        "rules": {
            "api.example.com": [
                {"transform": {"headers": {"X-A": "1"}}},
                {"transform": {"headers": {"Authorization": "Bearer ${e2b.identity.tokens.aws}"}}},
                {},
            ]
        },
        "allow_public_traffic": False,
        "mask_request_host": "${PORT}-app.example.com",
        "https_ports": [3000],
    }
    assert build_network_config({}) == {}
    assert build_network_config({"allow_out": ["1.1.1.1"]}) == {"allow_out": ["1.1.1.1"]}


def test_network_transform_callable_errors():
    with pytest.raises(InvalidArgumentException, match="Registered tokens: 'aws'"):
        build_network_config(
            {"rules": {"h": [{"transform": lambda c: {"headers": {"A": c.iam.tokens["gcp"]}}}]}},
            {"tokens": {"aws": {}}},
        )
    with pytest.raises(InvalidArgumentException) as info:
        build_network_config({"rules": {"h": [{"transform": lambda c: "nope"}]}})
    assert (
        str(info.value)
        == "Network transform callable for 'h' must return a transform dict, got str."
    )

    async def aw(ctx):
        return {}

    with pytest.raises(InvalidArgumentException) as info:
        build_network_config({"rules": {"h": [{"transform": aw}]}})
    assert str(info.value) == (
        "Network transform callable for 'h' must be synchronous, it returned an awaitable. "
        "Resolve the value before creating the sandbox."
    )


def test_egress_proxy_building_and_errors():
    body = build_network_config(
        {
            "egress_proxy": {
                "address": "p.example.com:1080",
                "username": "u",
                "password": "pw",
                "tls": {"server_name": "p.example.com", "ca_cert": "PEM"},
                "stray": 1,
            }
        }
    )
    assert body == {
        "egress_proxy": {
            "address": "p.example.com:1080",
            "username": "u",
            "password": "pw",
            "tls": {"enabled": True, "server_name": "p.example.com", "ca_cert": "PEM"},
        }
    }
    assert build_network_config({"egress_proxy": {"address": "a:1", "tls": True}})["egress_proxy"][
        "tls"
    ] == {"enabled": True}
    assert (
        "tls"
        not in build_network_config({"egress_proxy": {"address": "a:1", "tls": False}})[
            "egress_proxy"
        ]
    )
    for proxy in ({}, {"address": 5}):
        with pytest.raises(InvalidArgumentException) as info:
            build_network_config({"egress_proxy": proxy})
        assert str(info.value) == (
            "network egress_proxy must be a dict with a string 'address' (e.g. 'proxy.example.com:1080')."
        )
    with pytest.raises(InvalidArgumentException) as info:
        build_network_config({"egress_proxy": {"address": "a:1", "tls": {"enabled": True}}})
    assert (
        str(info.value)
        == "network egress_proxy tls takes no 'enabled'; pass tls=True or tls=False instead."
    )
    with pytest.raises(InvalidArgumentException) as info:
        build_network_config({"egress_proxy": {"address": "a:1", "tls": "yes"}})
    assert str(info.value) == "network egress_proxy tls must be a bool or a dict."


def test_build_network_update_body():
    assert build_network_update_body({}) == {}
    body = build_network_update_body(
        {
            "allow_out": ["a.com"],
            "deny_out": lambda ctx: [ctx.all_traffic],
            "allow_internet_access": False,
            # With no iam config at hand every token name resolves.
            "rules": {"a.com": [{"transform": lambda c: {"headers": {"T": c.iam.tokens["any"]}}}]},
        }
    )
    assert body == {
        "allow_out": ["a.com"],
        "deny_out": ["0.0.0.0/0"],
        "allow_internet_access": False,
        "rules": {"a.com": [{"transform": {"headers": {"T": "${e2b.identity.tokens.any}"}}}]},
    }


def test_from_client_network_and_lifecycle():
    assert from_client_network_config(e2b.api.client.UNSET) is None
    assert from_client_lifecycle(e2b.api.client.UNSET) is None
    info = from_client_network_config(
        {
            "allow_out": ("a",),
            "deny_out": ["0.0.0.0/0"],
            "rules": {"a": [{"transform": {"headers": {"X": "1"}}}]},
            "egress_proxy": {
                "address": "p:1",
                "username": "u",
                "password": "never-returned",
                "tls": {"enabled": True, "server_name": "p", "ca_cert": "PEM"},
            },
            "allow_public_traffic": True,
            "https_ports": (443,),
        }
    )
    assert info == {
        "allow_out": ["a"],
        "deny_out": ["0.0.0.0/0"],
        "rules": {"a": [{"transform": {"headers": {"X": "1"}}}]},
        "egress_proxy": {"address": "p:1", "username": "u", "tls": {"server_name": "p"}},
        "allow_public_traffic": True,
        "https_ports": [443],
    }
    assert from_client_network_config({"egress_proxy": None}) == {}
    assert from_client_network_config(
        {"egress_proxy": {"address": "p:1", "tls": {"enabled": False}}}
    ) == {"egress_proxy": {"address": "p:1"}}
    assert from_client_lifecycle({"on_timeout": "pause", "auto_resume": True}) == {
        "on_timeout": "pause",
        "auto_resume": True,
    }


def test_to_sandbox_create_response():
    got = to_sandbox_create_response(
        types.SimpleNamespace(
            sandbox_id="s",
            domain="e2b.app",
            envd_version="0.5.7",
            envd_access_token="tok",
            traffic_access_token=e2b.api.client.UNSET,
        )
    )
    assert (got.sandbox_id, got.sandbox_domain, got.envd_version) == ("s", "e2b.app", "0.5.7")
    assert (got.envd_access_token, got.traffic_access_token) == ("tok", None)
    assert (
        to_sandbox_create_response({"sandbox_id": "m", "envd_version": "1"}).sandbox_domain is None
    )


# -- the types behave ---------------------------------------------------------------------


def test_sandbox_info_and_query_fields():
    now = datetime.now(timezone.utc)
    info = SandboxInfo("s", None, "t", None, {}, now, now, e2b.SandboxState.RUNNING, 2, 512, "v")
    assert info.volume_mounts == [] and info.state == "running" and str(info.state) == "running"
    assert (
        SandboxInfo("s", None, "t", None, {}, now, now, "paused", 1, 1, "v").volume_mounts
        is not info.volume_mounts
    )
    q = SandboxQuery(metadata={"a": "b"}, started_after=now, template="base")
    assert (q.state, q.template) == (None, "base")
    assert e2b.SandboxState("paused") is e2b.SandboxState.PAUSED


def test_mcp_catalog_types():
    from e2b.sandbox import mcp

    assert mcp.Aks.__required_keys__ == {"accessLevel", "azureDir", "kubeconfig"}
    assert "additionalTools" in mcp.Aks.__optional_keys__
    assert mcp.Airtable.__required_keys__ == {"airtableApiKey", "nodeenv"}
    assert len(mcp.McpServer.__annotations__) > 150
    assert mcp.McpServer.__optional_keys__ == frozenset(mcp.McpServer.__annotations__)
    assert (
        e2b.McpServer is not mcp.McpServer
    )  # the public name also admits "github/owner/repo" keys


def test_network_constants():
    assert e2b.ALL_TRAFFIC == "0.0.0.0/0" == e2b.sandbox.network.ALL_TRAFFIC


# -- the moved definitions still work in the sandbox --------------------------------------


async def test_listing_uses_the_new_paginators(backend):
    a = await AsyncSandbox.create(template="world-1", metadata={"run": "a"})
    b = await AsyncSandbox.create(template="world-1", metadata={"run": "b"})
    pager = AsyncSandbox.list(query=SandboxQuery(metadata={"run": "b"}), limit=5)
    assert isinstance(pager, e2b.sandbox.sandbox_api.SandboxPaginatorBase)
    assert (pager.query.metadata, pager.limit, pager.order) == ({"run": "b"}, 5, None)
    items = await pager.next_items()
    assert [i.sandbox_id for i in items] == [b.sandbox_id]
    assert isinstance(items[0], SandboxInfo) and items[0].volume_mounts == []
    # One page holds everything: no cursor is ever issued.
    assert (pager.has_next, pager.next_token) == (False, None)
    with pytest.raises(Exception, match="No more items to fetch"):
        await pager.next_items()
    future = SandboxQuery(started_after=datetime(2999, 1, 1))
    assert await AsyncSandbox.list(query=future).next_items() == []
    assert len(await AsyncSandbox.list().next_items()) >= 2
    assert a.sandbox_id


async def test_paginator_constructors_have_e2bs_signatures(backend):
    from e2b.sandbox_async.paginator import (
        AsyncSandboxPaginator,
        AsyncSnapshotPaginator,
    )

    p = AsyncSandboxPaginator(None, 3, None, "desc")
    assert (p.limit, p.order) == (3, "desc")
    s = AsyncSnapshotPaginator("sbx", 4, None, "snap")
    assert (s.sandbox_id, s.limit, s.name) == ("sbx", 4, "snap")
    # a token this package did not issue is refused when the page is fetched,
    # with e2b's API text
    for make in (
        lambda: AsyncSandboxPaginator(next_token="abc"),
        lambda: AsyncSnapshotPaginator(next_token="abc"),
    ):
        with pytest.raises(e2b.SandboxException, match="^400: Invalid next token$"):
            await make().next_items()
    with pytest.raises(
        InvalidArgumentException, match="Invalid order 'sideways', expected 'asc' or 'desc'"
    ):
        await AsyncSandboxPaginator(order="sideways").next_items()
    # Per-call connection options are validated like e2b's.
    with pytest.raises(InvalidArgumentException, match="Invalid retries=-1"):
        await AsyncSandboxPaginator().next_items(retries=-1)


def test_class_method_variant_lives_in_sandbox_utils():
    from e2b.sandbox.utils import class_method_variant
    from e2b.sandbox_async.main import class_method_variant as same

    assert same is class_method_variant

    class Thing:
        @classmethod
        def _cls_ping(cls, name):
            return ("class", name)

        @class_method_variant("_cls_ping")
        def ping(self):
            return ("instance", "me")

    assert Thing().ping() == ("instance", "me")
    assert Thing.ping("x") == ("class", "x")
