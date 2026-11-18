"""Unit tests for core telemetry functionality.

All external dependencies are mocked. No test here may reach the network:
the PostHog client is always replaced with a mock and HOME points at a
temporary directory.
"""

import socket
import warnings
from unittest.mock import MagicMock, Mock, patch

import pytest
from cua_core.telemetry._config import CI_ENV_VARS, sanitize_model_name

TELEMETRY_ENV_VARS = (
    "DO_NOT_TRACK",
    "CUA_TELEMETRY",
    "CUA_TELEMETRY_ENABLED",
    "CUA_TELEMETRY_DISABLED",
    "CUA_HOME",
    *CI_ENV_VARS,
)


@pytest.fixture
def clean_env(monkeypatch, tmp_path):
    """Remove every telemetry/CI switch and point HOME at a temp dir."""
    for name in TELEMETRY_ENV_VARS:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    return tmp_path


@pytest.fixture
def mock_posthog_client(monkeypatch):
    """Replace posthog.Posthog with a mock and reset the singleton."""
    from cua_core.telemetry import posthog as ph_mod

    instance = MagicMock(name="PosthogClient")
    factory = MagicMock(name="Posthog", return_value=instance)
    monkeypatch.setattr(ph_mod.posthog, "Posthog", factory)
    ph_mod.PostHogTelemetryClient.destroy_client()
    yield factory, instance
    ph_mod.PostHogTelemetryClient.destroy_client()


@pytest.fixture
def no_network(monkeypatch):
    """Fail the test if anything tries to open a network connection."""

    def _blocked(*_args, **_kwargs):
        raise AssertionError("network access attempted during telemetry test")

    monkeypatch.setattr(socket.socket, "connect", _blocked)
    monkeypatch.setattr(socket.socket, "connect_ex", _blocked)
    monkeypatch.setattr(socket, "create_connection", _blocked)


class TestTelemetryEnabled:
    """Enablement rules."""

    def test_telemetry_enabled_by_default(self, clean_env):
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is True

    @pytest.mark.parametrize("value", ['"off"', "false", "0", '"disabled"'])
    def test_machine_setting_off_is_honoured(self, clean_env, monkeypatch, value):
        from cua_core.telemetry import is_telemetry_enabled

        cua_home = clean_env / ".cua"
        cua_home.mkdir()
        (cua_home / "config.toml").write_text(f"[telemetry]\nenabled = {value}\n")
        assert is_telemetry_enabled() is False

    def test_machine_setting_under_cua_home(self, clean_env, monkeypatch):
        from cua_core.telemetry import is_telemetry_enabled

        custom = clean_env / "custom-home"
        custom.mkdir()
        (custom / "config.toml").write_text('[telemetry]\nenabled = "off"\n')
        monkeypatch.setenv("CUA_HOME", str(custom))
        assert is_telemetry_enabled() is False

    def test_env_on_wins_over_machine_setting_off(self, clean_env, monkeypatch):
        from cua_core.telemetry import is_telemetry_enabled

        cua_home = clean_env / ".cua"
        cua_home.mkdir()
        (cua_home / "config.toml").write_text('[telemetry]\nenabled = "off"\n')
        monkeypatch.setenv("CUA_TELEMETRY", "1")
        assert is_telemetry_enabled() is True

    def test_unreadable_machine_setting_keeps_default(self, clean_env):
        from cua_core.telemetry import is_telemetry_enabled

        cua_home = clean_env / ".cua"
        cua_home.mkdir()
        (cua_home / "config.toml").write_text("not = [valid toml\n")
        assert is_telemetry_enabled() is True

    def test_telemetry_disabled_with_legacy_flag(self, clean_env, monkeypatch):
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "false")
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("value", ["0", "false", "no", "off", "OFF"])
    def test_legacy_enabled_falsy_values(self, clean_env, monkeypatch, value):
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", value)
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("value", ["1", "true", "yes", "on"])
    def test_legacy_enabled_truthy_values(self, clean_env, monkeypatch, value):
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", value)
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is True

    @pytest.mark.parametrize("value", ["0", "false", "no", "off"])
    def test_cua_telemetry_off(self, clean_env, monkeypatch, value):
        monkeypatch.setenv("CUA_TELEMETRY", value)
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("value", ["1", "true", "yes"])
    def test_do_not_track(self, clean_env, monkeypatch, value):
        monkeypatch.setenv("DO_NOT_TRACK", value)
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    def test_do_not_track_zero_or_empty_is_ignored(self, clean_env, monkeypatch):
        from cua_core.telemetry import is_telemetry_enabled

        monkeypatch.setenv("DO_NOT_TRACK", "0")
        assert is_telemetry_enabled() is True
        monkeypatch.setenv("DO_NOT_TRACK", "")
        assert is_telemetry_enabled() is True

    def test_do_not_track_wins_over_cua_telemetry_on(self, clean_env, monkeypatch):
        monkeypatch.setenv("DO_NOT_TRACK", "1")
        monkeypatch.setenv("CUA_TELEMETRY", "1")
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    def test_legacy_disabled_flag(self, clean_env, monkeypatch):
        monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "true")
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("ci_var", list(CI_ENV_VARS))
    def test_ci_defaults_off(self, clean_env, monkeypatch, ci_var):
        monkeypatch.setenv(ci_var, "true")
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("value", ["1", "on", "true"])
    def test_cua_telemetry_on_overrides_ci(self, clean_env, monkeypatch, value):
        monkeypatch.setenv("CI", "true")
        monkeypatch.setenv("GITHUB_ACTIONS", "true")
        monkeypatch.setenv("CUA_TELEMETRY", value)
        from cua_core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is True


class TestOtelEnabled:
    def test_otel_follows_same_rules(self, clean_env, monkeypatch):
        from cua_core.telemetry import otel

        assert otel.is_otel_enabled() is True
        monkeypatch.setenv("DO_NOT_TRACK", "1")
        assert otel.is_otel_enabled() is False
        monkeypatch.delenv("DO_NOT_TRACK")
        monkeypatch.setenv("CI", "1")
        assert otel.is_otel_enabled() is False
        monkeypatch.setenv("CUA_TELEMETRY", "on")
        assert otel.is_otel_enabled() is True

    def test_deprecated_flag_warns_once(self, clean_env, monkeypatch):
        monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "true")
        from cua_core.telemetry import otel

        monkeypatch.setattr(otel, "_deprecation_warning_emitted", False)
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            results = [otel.is_otel_enabled() for _ in range(3)]

        assert results == [False, False, False]
        assert len(caught) == 1
        assert caught[0].category is DeprecationWarning

    @pytest.mark.parametrize(
        "model,expected",
        [
            ("gpt-4o", "gpt-4o"),
            ("anthropic/claude-sonnet-4-5-20250929", "anthropic/claude-sonnet-4-5-20250929"),
            ("omniparser+openai/gpt-4o", "omniparser+openai/gpt-4o"),
            ("/Users/alice/models/finetune", "custom"),
            ("~/models/x", "custom"),
            ("C:\\Users\\alice\\model", "custom"),
            ("http://10.0.0.5:8000/v1", "custom"),
            ("alice/private-finetune", "custom"),
            ("huggingface-local/alice/private-model", "huggingface-local/custom"),
            ("x" * 65, "custom"),
            ("model with spaces", "custom"),
        ],
    )
    def test_model_label_sanitized(self, model, expected):
        assert sanitize_model_name(model) == expected

    def test_record_operation_sanitizes_and_drops_extra_attributes(self, clean_env, monkeypatch):
        from cua_core.telemetry import otel

        hist, counter = MagicMock(), MagicMock()
        monkeypatch.setattr(otel, "_initialize_otel", lambda: True)
        monkeypatch.setattr(otel, "_operation_duration", hist)
        monkeypatch.setattr(otel, "_operations_total", counter)

        otel.record_operation(
            "agent.step",
            0.5,
            model="/Users/alice/model.gguf",
            step_number=7,
            hostname="alice-mbp",
        )
        attrs = hist.record.call_args[0][1]
        assert attrs["model"] == "custom"
        assert "step_number" not in attrs
        assert "hostname" not in attrs
        assert set(attrs) <= otel.ALLOWED_ATTRIBUTE_KEYS

    def test_no_global_providers_installed(self, clean_env, monkeypatch):
        """_initialize_otel must not install global tracer/meter providers."""
        pytest.importorskip("opentelemetry.sdk")
        from cua_core.telemetry import otel
        from opentelemetry import metrics, trace

        set_meter = MagicMock()
        set_tracer = MagicMock()
        monkeypatch.setattr(metrics, "set_meter_provider", set_meter)
        monkeypatch.setattr(trace, "set_tracer_provider", set_tracer)
        # Exporters would reach the network on export; replace them.
        import opentelemetry.exporter.otlp.proto.http.metric_exporter as me
        import opentelemetry.exporter.otlp.proto.http.trace_exporter as te

        monkeypatch.setattr(me, "OTLPMetricExporter", MagicMock())
        monkeypatch.setattr(te, "OTLPSpanExporter", MagicMock())
        monkeypatch.setattr(otel.atexit, "register", MagicMock())
        for name in ("_initialized", "_init_failed"):
            monkeypatch.setattr(otel, name, False)
        for name in ("_meter", "_tracer", "_meter_provider", "_tracer_provider"):
            monkeypatch.setattr(otel, name, None)

        assert otel._initialize_otel() is True
        set_meter.assert_not_called()
        set_tracer.assert_not_called()
        # Shut down the module-local providers; the exporter is a mock.
        otel._shutdown_otel()


class TestPostHogTelemetryClient:
    def test_client_initialization(self, clean_env, mock_posthog_client):
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        client = PostHogTelemetryClient()
        assert client.initialized is True
        assert client.installation_id is not None
        assert len(client.installation_id) == 36

    def test_uses_dedicated_client_with_geoip_disabled(self, clean_env, mock_posthog_client):
        import posthog
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        factory, _ = mock_posthog_client
        before = (getattr(posthog, "api_key", None), getattr(posthog, "host", None))
        PostHogTelemetryClient()
        factory.assert_called_once()
        assert factory.call_args.kwargs["disable_geoip"] is True
        assert (getattr(posthog, "api_key", None), getattr(posthog, "host", None)) == before

    def test_installation_id_persistence(self, clean_env, mock_posthog_client):
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        id_file = clean_env / ".config" / "cua" / "installation_id"
        id_file.parent.mkdir(parents=True)
        id_file.write_text("test-installation-id-123\n")
        client = PostHogTelemetryClient()
        assert client.installation_id == "test-installation-id-123"

    def test_installation_id_created_when_enabled(self, clean_env, mock_posthog_client):
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        client = PostHogTelemetryClient()
        id_file = clean_env / ".config" / "cua" / "installation_id"
        assert id_file.read_text() == client.installation_id

    @pytest.mark.parametrize(
        "var,value",
        [
            ("DO_NOT_TRACK", "1"),
            ("CUA_TELEMETRY", "0"),
            ("CUA_TELEMETRY_ENABLED", "false"),
            ("CI", "true"),
        ],
    )
    def test_no_id_file_when_disabled(
        self, clean_env, mock_posthog_client, monkeypatch, var, value
    ):
        from cua_core.telemetry.posthog import PostHogTelemetryClient, record_event

        monkeypatch.setenv(var, value)
        factory, instance = mock_posthog_client
        client = PostHogTelemetryClient()
        client.record_event("test_event", {"key": "value"})
        record_event("test_event")
        client.flush()

        assert not (clean_env / ".config").exists()
        assert client.installation_id is None
        factory.assert_not_called()
        instance.capture.assert_not_called()

    def test_record_event_payload(self, clean_env, mock_posthog_client, no_network):
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        _, instance = mock_posthog_client
        client = PostHogTelemetryClient()
        client.record_event("test_event", {"key": "value"})

        assert instance.capture.call_count == 1
        kwargs = instance.capture.call_args.kwargs
        assert kwargs["event"] == "test_event"
        assert kwargs["distinct_id"] == client.installation_id
        assert kwargs["disable_geoip"] is True
        props = kwargs["properties"]
        assert props["$process_person_profile"] is False
        assert props["key"] == "value"
        assert "version" in props

    def test_no_identify_event(self, clean_env, mock_posthog_client):
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        _, instance = mock_posthog_client
        client = PostHogTelemetryClient()
        client.record_event("a")
        client.record_event("b")
        events = [c.kwargs["event"] for c in instance.capture.call_args_list]
        assert events == ["a", "b"]
        assert not hasattr(client, "_identify")
        instance.identify.assert_not_called()

    def test_record_events_never_touch_network(self, clean_env, mock_posthog_client, no_network):
        from cua_core.telemetry import record_event
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        _, instance = mock_posthog_client
        for i in range(5):
            record_event("evt", {"i": i})
        PostHogTelemetryClient.get_client().flush()
        assert instance.capture.call_count == 5

    def test_no_sensitive_values_logged_at_info(self, clean_env, mock_posthog_client, caplog):
        import logging

        from cua_core.telemetry.posthog import (
            PUBLIC_POSTHOG_API_KEY,
            PostHogTelemetryClient,
        )

        with caplog.at_level(logging.INFO, logger="core.telemetry"):
            client = PostHogTelemetryClient()
            client.record_event("evt", {"secret_marker": "zzz-marker"})
        text = caplog.text
        assert client.installation_id not in text
        assert PUBLIC_POSTHOG_API_KEY not in text
        assert "zzz-marker" not in text

    def test_singleton_pattern(self, clean_env, mock_posthog_client):
        from cua_core.telemetry.posthog import PostHogTelemetryClient

        assert PostHogTelemetryClient.get_client() is PostHogTelemetryClient.get_client()


class TestRecordEvent:
    @patch("cua_core.telemetry.posthog.PostHogTelemetryClient")
    def test_record_event_calls_client(self, mock_client_class, disable_telemetry):
        from cua_core.telemetry import record_event

        mock_client_instance = Mock()
        mock_client_class.get_client.return_value = mock_client_instance
        record_event("test_event", {"key": "value"})
        mock_client_instance.record_event.assert_called_once_with("test_event", {"key": "value"})

    @patch("cua_core.telemetry.posthog.PostHogTelemetryClient")
    def test_record_event_without_properties(self, mock_client_class, disable_telemetry):
        from cua_core.telemetry import record_event

        mock_client_instance = Mock()
        mock_client_class.get_client.return_value = mock_client_instance
        record_event("test_event")
        mock_client_instance.record_event.assert_called_once_with("test_event", {})


class TestDestroyTelemetryClient:
    @patch("cua_core.telemetry.posthog.PostHogTelemetryClient")
    def test_destroy_client_calls_class_method(self, mock_client_class):
        from cua_core.telemetry import destroy_telemetry_client

        destroy_telemetry_client()
        mock_client_class.destroy_client.assert_called_once()
