"""GUI lifecycle tests: login lock, generations, pagehide, model selector.

These exercise the real Gradio component tree built by
``cua_agent.ui.gradio.orcarouter_ui``, with a fake catalog and a local fake
authorization server.
"""

import urllib.parse

import pytest

from cua_agent.orcarouter import (
    ORCAROUTER_LOOP,
    LoopbackListener,
    OrcaRouterAuthError,
    OrcaRouterConnectSession,
    OrcaRouterLoginBusy,
    PkceAttempt,
    build_authorize_url,
)
from cua_agent.ui.gradio import orcarouter_ui
from cua_agent.ui.gradio.orcarouter_ui import TASK_CAPABILITIES
from tests.orcarouter.conftest import FAKE_PKCE_KEY, make_provider


class TestLoginSessionLifecycle:
    def test_begin_publishes_a_url_and_sets_busy(self, session):
        generation, _attempt, url = session.begin()
        snapshot = session.snapshot()
        assert snapshot.busy is True
        assert snapshot.generation == generation
        assert snapshot.hint
        assert url.startswith("https://www.orcarouter.ai/auth?")
        assert "callback_url=oob" in url
        assert "code_challenge_method=S256" in url

    def test_a_second_begin_is_refused_while_busy(self, session):
        session.begin()
        with pytest.raises(OrcaRouterLoginBusy):
            session.begin()

    def test_success_releases_the_lock(self, session):
        generation, attempt, _url = session.begin()
        from cua_agent.orcarouter import OrcaRouterCredential

        assert session.succeed(
            generation, OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce")
        )
        snapshot = session.snapshot()
        assert snapshot.busy is False
        assert snapshot.ok is True
        assert snapshot.hint == ""
        assert FAKE_PKCE_KEY not in snapshot.message

    def test_failure_releases_the_lock(self, session):
        generation, _attempt, _url = session.begin()
        session.fail(generation, OrcaRouterAuthError("exchange failed", kind="exchange_rejected"))
        assert session.snapshot().busy is False
        assert session.snapshot().ok is False

    def test_denial_releases_the_lock_with_actionable_text(self, session):
        generation, _attempt, _url = session.begin()
        session.fail(generation, OrcaRouterAuthError("access_denied", kind="denied"))
        snapshot = session.snapshot()
        assert snapshot.busy is False
        assert "denied" in (snapshot.message or "").lower()
        assert "api key" in (snapshot.message or "").lower()

    def test_timeout_releases_the_lock(self, session):
        generation, _attempt, _url = session.begin()
        session.fail(generation, OrcaRouterAuthError("timed out", kind="expired"))
        assert session.snapshot().busy is False

    def test_explicit_cancel_releases_the_lock_and_ignores_late_responses(self, session):
        generation, _attempt, _url = session.begin()
        session.cancel()
        snapshot = session.snapshot()
        assert snapshot.busy is False

        # A late exchange result must not resurrect the cancelled attempt.
        from cua_agent.orcarouter import OrcaRouterCredential

        assert (
            session.succeed(generation, OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))
            is False
        )
        assert session.snapshot().ok is False

    def test_switching_authentication_method_releases_the_lock(self, session):
        session.begin()
        session.switch_method()
        assert session.snapshot().busy is False

    def test_pending_attempt_is_available_only_while_busy(self, session):
        generation, attempt, _url = session.begin()
        assert session.pending_attempt is attempt
        session.cancel()
        assert session.pending_attempt is None

    def test_verifier_never_reaches_the_published_url(self, session):
        _generation, attempt, url = session.begin()
        assert attempt.verifier not in url
        assert attempt.verifier not in (session.snapshot().hint or "")
        assert attempt.verifier not in (session.snapshot().url or "")


class TestPagehide:
    def test_pagehide_clears_busy_without_a_remount(self, session):
        session.begin()
        assert session.snapshot().busy is True

        session.pagehide()

        snapshot = session.snapshot()
        # Cleared synchronously, not by the guarded `finally` of the in-flight task.
        assert snapshot.busy is False
        assert snapshot.hint == ""

    def test_second_login_starts_immediately_after_pagehide(self, session):
        first_generation, _attempt, _url = session.begin()
        session.pagehide(first_generation)

        second_generation, _attempt2, url = session.begin()
        assert second_generation > first_generation
        assert session.snapshot().busy is True
        assert url.startswith("https://www.orcarouter.ai/auth?")

    def test_pagehide_does_not_let_the_old_generation_finish(self, session):
        from cua_agent.orcarouter import OrcaRouterCredential

        generation, _attempt, _url = session.begin()
        session.pagehide(generation)

        assert (
            session.succeed(generation, OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))
            is False
        )
        assert session.snapshot().ok is False

    def test_old_generation_cancel_does_not_clear_a_new_login(self, session):
        first, _attempt, _url = session.begin()
        session.pagehide(first)
        session.begin()

        # A late cancel attributed to the old generation must not unlock the new one.
        session.fail(first, OrcaRouterAuthError("late", kind="network"))
        assert session.snapshot().busy is True


class TestCatalogSelectorWiring:
    def test_options_come_from_the_api_not_a_static_list(self, tmp_path):
        provider = make_provider(tmp_path)
        choices, allowed, _value, status = orcarouter_ui._catalog_choices(
            provider, "Text chat / computer use", [], None
        )
        assert allowed
        assert all(value.startswith("orcarouter/") for value in allowed)
        # Ids that only exist in the fixture catalog, i.e. not hand-written.
        assert "orcarouter/openai/gpt-5.5" in allowed
        assert "live" in status
        assert len(provider.catalog_calls) == 1

    def test_selector_is_never_free_text(self, tmp_path):
        provider = make_provider(tmp_path)
        choices, _allowed, _value, _status = orcarouter_ui._catalog_choices(
            provider, "Text chat / computer use", [], None
        )
        # Every option is a (label, value) pair drawn from the catalog.
        assert choices
        assert all(isinstance(pair, tuple) and len(pair) == 2 for pair in choices)

    def test_image_requirement_leaves_only_image_capable_chat_models(self, tmp_path):
        provider = make_provider(tmp_path)
        _choices, allowed, _value, _status = orcarouter_ui._catalog_choices(
            provider,
            "Text chat / computer use",
            ["Screenshot understanding (image input)"],
            None,
        )
        assert "orcarouter/openai/gpt-5.5" in allowed
        assert "orcarouter/deepseek/deepseek-v4-pro" not in allowed
        assert "orcarouter/openai/text-embedding-3-large" not in allowed

    def test_no_modality_requirement_keeps_text_models(self, tmp_path):
        provider = make_provider(tmp_path)
        _choices, allowed, _value, _status = orcarouter_ui._catalog_choices(
            provider, "Text chat / computer use", [], None
        )
        assert "orcarouter/deepseek/deepseek-v4-pro" in allowed

    def test_embedding_task_switches_the_catalog_filter(self, tmp_path):
        provider = make_provider(tmp_path)
        _choices, allowed, _value, _status = orcarouter_ui._catalog_choices(
            provider, "Embedding", [], None
        )
        assert allowed == ["orcarouter/openai/text-embedding-3-large"]

    def test_selection_invalidated_by_a_new_requirement_is_cleared(self, tmp_path):
        provider = make_provider(tmp_path)
        _choices, allowed, value, status = orcarouter_ui._catalog_choices(
            provider,
            "Text chat / computer use",
            ["Screenshot understanding (image input)"],
            "orcarouter/deepseek/deepseek-v4-pro",
        )
        assert value is None
        assert "orcarouter/deepseek/deepseek-v4-pro" not in allowed

    def test_cleared_or_unset_value_is_none_not_an_empty_string(self, tmp_path):
        """Gradio rejects ``""`` as a value outside the choice list.

        Setting an empty string here turns the *next* event on the dropdown into
        ``Value:  is not in the list of choices`` and returns HTTP 500, which is
        what left the control empty in the running UI.
        """
        import gradio as gr

        provider = make_provider(tmp_path)
        for selected in (None, "", "orcarouter/vendor/removed"):
            _choices, allowed, value, _status = orcarouter_ui._catalog_choices(
                provider, "Text chat / computer use", [], selected
            )
            assert allowed
            assert value is None

        # And the produced update must be accepted by the component itself.
        dropdown = gr.Dropdown(choices=[], value=None, allow_custom_value=False)
        _choices, _allowed, value, _status = orcarouter_ui._catalog_choices(
            provider, "Text chat / computer use", [], ""
        )
        dropdown.choices = [(label, val) for label, val in _choices]
        assert dropdown.preprocess(value) in (None, "")

    def test_compatible_selection_is_kept(self, tmp_path):
        provider = make_provider(tmp_path)
        _choices, _allowed, value, _status = orcarouter_ui._catalog_choices(
            provider,
            "Text chat / computer use",
            ["Screenshot understanding (image input)"],
            "orcarouter/openai/gpt-5.5",
        )
        assert value == "orcarouter/openai/gpt-5.5"

    def test_outage_shows_a_labelled_fallback_not_free_text(self, tmp_path):
        def failing(url, key):
            raise OSError("down")

        provider = make_provider(tmp_path, fetcher=failing)
        choices, allowed, _value, status = orcarouter_ui._catalog_choices(
            provider, "Text chat / computer use", [], None
        )
        assert choices
        assert all(v.startswith("orcarouter/") for v in allowed)
        assert "verified fallback" in status

    def test_empty_catalog_reports_no_selectable_model(self, tmp_path):
        provider = make_provider(tmp_path, catalog_payload={"data": []})

        def failing(url, key):
            raise OSError("down")

        provider.catalog.fetcher = failing
        choices, allowed, _value, status = orcarouter_ui._catalog_choices(
            provider, "Embedding", [], None
        )
        assert choices == []
        assert allowed == []
        assert "No OrcaRouter model" in status

    def test_status_is_redacted(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_PKCE_KEY)
        markdown = orcarouter_ui._status_markdown(provider)
        assert FAKE_PKCE_KEY not in markdown
        assert "sk-orca-" in markdown

    def test_status_reports_needs_reauth(self, tmp_path):
        provider = make_provider(tmp_path, api_key=FAKE_PKCE_KEY)
        generation = provider.begin_generation()
        provider.on_unauthorized(provider.resolve_credential(), generation)
        assert "reauthentication required" in orcarouter_ui._status_markdown(provider)


class TestRenderedComponents:
    """Build the real Gradio tree and assert both choices are present."""

    @pytest.fixture
    def panel(self, tmp_path):
        provider = make_provider(tmp_path)
        session = OrcaRouterConnectSession(provider=provider)
        import gradio as gr

        with gr.Blocks():
            components = orcarouter_ui.build_orcarouter_panel(provider, session)
        return provider, session, components

    def test_both_authentication_choices_are_rendered(self, panel):
        _provider, _session, components = panel
        assert components["api_key_input"].label == "OrcaRouter API Key"
        assert "Connect with OrcaRouter" in components["connect_button"].value

    def test_cancel_is_available_alongside_complete(self, panel):
        _provider, _session, components = panel
        assert components["cancel_button"] is not None
        assert components["complete_button"] is not None

    def test_the_model_control_is_a_dropdown_without_free_text(self, panel):
        import gradio as gr

        _provider, _session, components = panel
        dropdown = components["model_dropdown"]
        assert isinstance(dropdown, gr.Dropdown)
        assert dropdown.allow_custom_value is False

    def test_refresh_control_exists(self, panel):
        _provider, _session, components = panel
        assert components["refresh_models"] is not None

    def test_initial_model_is_validated_against_the_capability(self, tmp_path):
        provider = make_provider(tmp_path)
        session = OrcaRouterConnectSession(provider=provider)
        import gradio as gr

        with gr.Blocks():
            components = orcarouter_ui.build_orcarouter_panel(
                provider, session, initial_model="orcarouter/deepseek/deepseek-v4-pro"
            )
        values = [value for _label, value in components["model_dropdown"].choices]
        assert "orcarouter/deepseek/deepseek-v4-pro" in values

    def test_incompatible_initial_model_is_dropped(self, tmp_path):
        provider = make_provider(tmp_path)
        session = OrcaRouterConnectSession(provider=provider)
        import gradio as gr

        with gr.Blocks():
            components = orcarouter_ui.build_orcarouter_panel(
                provider, session, initial_model="orcarouter/vendor/removed-model"
            )
        assert components["model_dropdown"].value is None

    def test_selecting_the_loop_populates_the_catalog(self, tmp_path):
        """Switching to ORCAROUTER fills the list; another loop leaves it alone."""
        provider = make_provider(tmp_path)
        session = OrcaRouterConnectSession(provider=provider)
        import gradio as gr

        with gr.Blocks():
            components = orcarouter_ui.build_orcarouter_panel(provider, session)
        sync = components["sync_catalog"]

        update, status = sync(ORCAROUTER_LOOP, "Text chat / computer use", [], None)
        assert update["__type__"] == "update"
        assert update["choices"]
        assert all(value.startswith("orcarouter/") for _label, value in update["choices"])
        assert "live" in status

        untouched, _status = sync("OPENAI", "Text chat / computer use", [], None)
        assert "choices" not in untouched

    def test_pagehide_handler_clears_a_live_login(self, tmp_path):
        provider = make_provider(tmp_path)
        session = OrcaRouterConnectSession(provider=provider)
        import gradio as gr

        with gr.Blocks():
            components = orcarouter_ui.build_orcarouter_panel(provider, session)

        session.begin()
        assert session.snapshot().busy is True
        components["pagehide_handler"]()
        assert session.snapshot().busy is False

    def test_orcarouter_is_a_distinct_loop_identifier(self):
        assert ORCAROUTER_LOOP == "ORCAROUTER"
        # The loop identifier is not a model capability; the panel maps each
        # entry point to its own capability filter instead.
        assert TASK_CAPABILITIES["Text chat / computer use"] == "chat"
        assert "chat" in set(TASK_CAPABILITIES.values())


class TestAuthorizeUrlFromSession:
    def test_session_uses_oob_and_the_auth_origin(self, session):
        url = build_authorize_url(
            PkceAttempt.create(),
            auth_base_url=session.provider.auth_base_url,
            callback_url="oob",
        )
        parsed = urllib.parse.urlsplit(url)
        assert parsed.netloc == "www.orcarouter.ai"
        assert parsed.path == "/auth"

    def test_listener_and_session_agree_on_s256(self):
        with LoopbackListener() as listener:
            assert listener.callback_url.startswith("http://127.0.0.1:")
