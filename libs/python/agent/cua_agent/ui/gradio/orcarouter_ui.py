"""OrcaRouter panel for the Gradio agent UI.

Presents the two authentication choices side by side and drives the model
control from the live OrcaRouter catalog. Both choices write the same secret
slot, so switching between them needs no other change.

The model dropdown is never free text: after OrcaRouter is selected the options
come from ``GET {api_base}/models`` filtered by capability. Adding a
multimodal requirement (or changing the attachment requirement) recomputes the
list and clears a selection that is no longer compatible.
"""

from __future__ import annotations

from typing import Any, Callable

import gradio as gr

from ...orcarouter import (
    ORCAROUTER_LOOP,
    OrcaRouterAuthError,
    OrcaRouterConnectSession,
    OrcaRouterLoginBusy,
    OrcaRouterProvider,
    exchange_url,
    post_exchange,
)

#: Capability filters the panel can request, keyed by the UI task choice.
TASK_CAPABILITIES: dict[str, str] = {
    "Text chat / computer use": "chat",
    "Embedding": "embedding",
    "Image generation": "image",
    "Video generation": "video",
    "Rerank": "rerank",
}

#: Input modalities a task can require; an undeclared modality fails closed.
TASK_MODALITIES: dict[str, tuple[str, ...]] = {
    "Screenshot understanding (image input)": ("image",),
    "Audio input": ("audio",),
    "Video input": ("video",),
}


def _status_markdown(provider: OrcaRouterProvider) -> str:
    status = provider.status()
    if status.needs_reauth:
        return "**OrcaRouter: reauthentication required.** " + provider.reauth_message()
    if status.source is None:
        return "**OrcaRouter:** no credential yet. Use either choice below."
    label = "API key" if status.source == "api_key" else "Connect with OrcaRouter"
    return f"**OrcaRouter:** configured via {label} — `{status.masked}`"


def _catalog_choices(
    provider: OrcaRouterProvider,
    task: str,
    modalities: list[str],
    selected: str | None,
) -> tuple[list[tuple[str, str]], list[str], str | None, str]:
    """Return ``(dropdown_choices, allowed_ids, value, status_text)``."""
    capability = TASK_CAPABILITIES.get(task, "chat")
    required: tuple[str, ...] = ()
    if capability == "chat":
        for label, mods in TASK_MODALITIES.items():
            if label in (modalities or []):
                required += mods

    result = provider.load_catalog(capability=capability, required_input_modalities=required)
    choices: list[tuple[str, str]] = []
    for model in result.models:
        details = []
        if model.context_length:
            details.append(f"ctx {model.context_length}")
        if model.input_modalities:
            details.append("/".join(model.input_modalities))
        if model.reasoning_efforts:
            details.append("reasoning: " + ",".join(model.reasoning_efforts))
        label = f"orcarouter/{model.id}"
        if details:
            label += f"  [{' | '.join(details)}]"
        choices.append((label, f"orcarouter/{model.id}"))

    allowed = [value for _label, value in choices]
    # An unset dropdown value must be ``None``: Gradio rejects ``""`` as a value
    # that is not among the choices, which turns the next event into a 500.
    value = selected if selected in allowed else None
    if not choices:
        status = "No OrcaRouter model matches this capability. Nothing is selectable."
    elif result.degraded:
        source = "last known-good catalog" if result.from_cache else "verified fallback models"
        status = (
            f"Live discovery unavailable ({result.degraded_reason}). Showing the "
            f"{source}; use Refresh to retry."
        )
    else:
        status = f"{len(choices)} live OrcaRouter models for {capability}."
    return choices, allowed, value, status


def build_orcarouter_panel(
    provider: OrcaRouterProvider,
    session: OrcaRouterConnectSession,
    initial_model: str | None = None,
    initial_task: str = "Text chat / computer use",
) -> dict[str, Any]:
    """Build the OrcaRouter group and return its components by name."""
    components: dict[str, Any] = {}

    def recompute_view(task: str, modalities: list[str], selected: str | None) -> tuple[Any, str]:
        """Rebuild the model control from the catalog for the current filters."""
        choices, _allowed, value, status = _catalog_choices(provider, task, modalities, selected)
        return gr.update(choices=choices, value=value), status

    with gr.Group(visible=False) as panel:
        components["panel"] = panel
        gr.Markdown(
            "OrcaRouter is an OpenAI-compatible AI gateway that routes many "
            "providers behind one endpoint. Choose **either** authentication "
            "method — both produce the same OrcaRouter API key."
        )
        status_view = gr.Markdown(_status_markdown(provider))

        with gr.Row():
            with gr.Column():
                gr.Markdown("**OrcaRouter - API** — paste an existing key")
                api_key_input = gr.Textbox(
                    label="OrcaRouter API Key",
                    placeholder="sk-orca-...",
                    type="password",
                    info=(
                        "Stored as ORCA_KEY in this project's .env (gitignored) and "
                        "passed to the model provider. Manage keys at "
                        "https://www.orcarouter.ai/console/authorized-apps"
                    ),
                )
                with gr.Row():
                    save_key_button = gr.Button("Save key", variant="primary")
                    clear_key_button = gr.Button("Clear key")

            with gr.Column():
                gr.Markdown(
                    "**OrcaRouter - Auth** — authorize with your OrcaRouter account "
                    "(OAuth 2.0 + PKCE)"
                )
                connect_button = gr.Button("Connect with OrcaRouter", variant="primary")
                auth_hint = gr.Markdown("")
                auth_url = gr.Textbox(
                    label="Authorization URL (copy if no browser opened)",
                    interactive=False,
                )
                code_input = gr.Textbox(
                    label="Authorization code",
                    placeholder="Paste the code OrcaRouter displayed",
                    info="PKCE binds this code to this process; it expires after 10 minutes.",
                )
                with gr.Row():
                    complete_button = gr.Button("Complete connection")
                    cancel_button = gr.Button("Cancel", variant="stop")

        with gr.Row():
            task_choice = gr.Radio(
                choices=list(TASK_CAPABILITIES),
                value=initial_task,
                label="Model capability",
                info="Filters the model list for this entry point.",
            )
            modality_choice = gr.CheckboxGroup(
                choices=list(TASK_MODALITIES),
                value=[],
                label="Required input modalities",
                info=(
                    "Only models that explicitly declare the modality are offered; "
                    "others are excluded."
                ),
            )
            refresh_models = gr.Button("Refresh models")

        model_dropdown = gr.Dropdown(
            label="OrcaRouter Model",
            choices=[],
            value=None,
            interactive=True,
            allow_custom_value=False,
            info="Options come from the live OrcaRouter model catalog.",
            elem_classes=["orcarouter-model-dropdown"],
        )
        # The catalog popup is the proof that the options are real, so it gets an
        # explicit opaque surface and border instead of the stock transparent one.
        gr.HTML(
            "<style>"
            ".orcarouter-model-dropdown .options {"
            "  background: var(--background-fill-primary, #ffffff);"
            "  border: 1px solid var(--border-color-primary, #d4d4d8);"
            "  border-radius: 6px;"
            "}"
            "</style>"
        )
        catalog_status = gr.Markdown("")

        # -- API key choice ------------------------------------------------

        def apply_api_key(key: str):
            if key and key.strip():
                status = provider.store_api_key(key.strip())
                return _status_markdown(provider), f"Stored ({status.masked})."
            return _status_markdown(provider), "Enter an OrcaRouter API key first."

        save_key_button.click(
            fn=apply_api_key,
            inputs=[api_key_input],
            outputs=[status_view, catalog_status],
            queue=False,
        ).then(
            fn=recompute_view,
            inputs=[task_choice, modality_choice, model_dropdown],
            outputs=[model_dropdown, catalog_status],
            queue=False,
        )

        def clear_key():
            provider.clear_credential()
            session.cancel(reason="Credential cleared.")
            return _status_markdown(provider), "", ""

        clear_key_button.click(
            fn=clear_key,
            inputs=None,
            outputs=[status_view, api_key_input, code_input],
            queue=False,
        )

        # -- PKCE choice ---------------------------------------------------

        def start_connect():
            try:
                generation, _attempt, url = session.begin()
            except OrcaRouterLoginBusy as error:
                snapshot = session.snapshot()
                return (
                    gr.update(),  # keep the current URL
                    str(error),
                    snapshot.hint,
                    session.generation,
                )
            return url, "", session.snapshot().hint, generation

        connect_state = gr.State(value=0)
        connect_button.click(
            fn=start_connect,
            inputs=None,
            outputs=[auth_url, catalog_status, auth_hint, connect_state],
            queue=False,
        )

        def complete_connection(code: str, generation: int):
            """Exchange the pasted code using this process's verifier."""
            if session.is_cancelled(generation):
                return (
                    _status_markdown(provider),
                    "That authorization attempt is no longer active. Start again.",
                    "",
                )
            attempt = session.pending_attempt
            if attempt is None:
                return (
                    _status_markdown(provider),
                    "No authorization attempt is pending.",
                    "",
                )
            if not (code or "").strip():
                return (
                    _status_markdown(provider),
                    "Paste the code OrcaRouter displayed first.",
                    code,
                )
            try:
                result = post_exchange(exchange_url(provider.auth_base_url), attempt, code.strip())
            except OrcaRouterAuthError as error:
                session.fail(generation, error)
                return _status_markdown(provider), str(error), ""
            credential = result.to_credential()
            if credential.granted_scope and credential.granted_scope != "api":
                session.fail(
                    generation,
                    OrcaRouterAuthError(
                        f"OrcaRouter granted the scope {credential.granted_scope!r}, which "
                        f"this client does not use; the key was not stored.",
                        kind="scope_downgrade",
                    ),
                )
                return _status_markdown(provider), session.snapshot().message or "", ""
            provider.pkce_store.save(credential)
            provider.begin_generation()
            session.succeed(generation, credential)
            return _status_markdown(provider), session.snapshot().message or "", ""

        complete_button.click(
            fn=complete_connection,
            inputs=[code_input, connect_state],
            outputs=[status_view, auth_hint, code_input],
            queue=True,
        )

        def cancel_connect():
            session.cancel()
            return "", session.snapshot().message or "", ""

        cancel_button.click(
            fn=cancel_connect,
            inputs=None,
            outputs=[auth_url, auth_hint, code_input],
            queue=False,
        )

        def on_pagehide():
            """Back-forward cache: clear busy/hint synchronously, then cancel."""
            session.pagehide()
            return "", session.snapshot().message or ""

        # `pagehide` is a real browser event dispatched from the page; it is wired
        # through the panel's unload handler so a restored page can start again.
        components["pagehide_handler"] = on_pagehide

        # -- catalog -------------------------------------------------------

        def sync_catalog(loop: str | None, task: str, modalities: list[str], selected: str | None):
            """Populate the model list as soon as the OrcaRouter loop is active.

            Selecting the loop must never leave an empty (or free-text) control,
            so the catalog is fetched here rather than only on Refresh.
            """
            if loop and loop != ORCAROUTER_LOOP:
                return gr.update(), gr.update()
            return recompute_view(task, modalities, selected)

        for component in (task_choice, modality_choice):
            component.change(
                fn=recompute_view,
                inputs=[task_choice, modality_choice, model_dropdown],
                outputs=[model_dropdown, catalog_status],
                queue=False,
            )

        refresh_models.click(
            fn=lambda task, mods, value: (
                provider.catalog.invalidate() if provider.catalog else None,
                recompute_view(task, mods, value),
            )[1],
            inputs=[task_choice, modality_choice, model_dropdown],
            outputs=[model_dropdown, catalog_status],
            queue=False,
        )

        components.update(
            {
                "status_view": status_view,
                "api_key_input": api_key_input,
                "save_key": save_key_button,
                "clear_key": clear_key_button,
                "connect_button": connect_button,
                "auth_hint": auth_hint,
                "auth_url": auth_url,
                "code_input": code_input,
                "complete_button": complete_button,
                "cancel_button": cancel_button,
                "task_choice": task_choice,
                "modality_choice": modality_choice,
                "refresh_models": refresh_models,
                "model_dropdown": model_dropdown,
                "catalog_status": catalog_status,
                "sync_catalog": sync_catalog,
            }
        )

        if initial_model:
            choices, _allowed, value, status = _catalog_choices(
                provider, initial_task, [], initial_model
            )
            model_dropdown.choices = choices
            model_dropdown.value = value
            catalog_status.value = status

    return components
