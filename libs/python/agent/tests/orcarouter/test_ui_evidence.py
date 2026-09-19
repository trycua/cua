"""GUI evidence: drive the real Gradio app and capture the OrcaRouter panel.

This is the only test that renders the product interface. It starts the real
``create_gradio_ui`` app on a loopback port, drives it with Playwright and
``/usr/bin/chromium``, and writes ``orca-evidence/`` at the repository root with
the screenshots and the assertions the delivery gate re-checks.

It needs a live credential: the model lists in the captures have to be the ones
this key can actually call, so the test is skipped when
``ORCAROUTER_API_KEY`` is absent. The secret is never written to the manifest,
the screenshots or the log — the panel is asserted to show only the masked
rendering.
"""

from __future__ import annotations

import hashlib
import json
import os
import socket
import time
from pathlib import Path

import pytest

pytest.importorskip("gradio", reason="the Gradio UI extra is required")
pytest.importorskip("playwright.sync_api", reason="Playwright is required")

from cua_agent.orcarouter import (  # noqa: E402
    OrcaRouterConnectSession,
    install_orcarouter_provider,
)
from cua_agent.ui.gradio.ui_components import create_gradio_ui  # noqa: E402

#: The authoritative chat catalog the capture must reflect.
CATALOG_SOURCE = "https://api.orcarouter.ai/v1/models?capability=chat"
CHROMIUM = "/usr/bin/chromium"
VIEWPORT = {"width": 1440, "height": 1100}


def _repository_root() -> Path:
    """The checkout that contains this test, found by its ``.git`` marker."""
    for parent in Path(__file__).resolve().parents:
        if (parent / ".git").exists():
            return parent
    raise RuntimeError("could not locate the repository root above this test")


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _trigger_box(page, label: str) -> dict:
    """The control's own box: the ``.wrap`` element Gradio renders around it."""
    return page.evaluate(
        """(label) => {
      const input = document.querySelector(`input[aria-label='${label}']`);
      const wrap = input.closest('.wrap');
      const r = wrap.getBoundingClientRect();
      return {x: r.x, y: r.y, width: r.width, height: r.height};
    }""",
        label,
    )


def _open_option_list(page) -> dict | None:
    """Measure the expanded option list: size, container styling, items."""
    return page.evaluate(
        """() => {
      const ul = [...document.querySelectorAll('ul.option-list')].find(
        (e) => e.getBoundingClientRect().height > 0);
      if (!ul) return null;
      const box = ul.getBoundingClientRect();
      const holder = ul.parentElement;
      const scale = holder.getBoundingClientRect().width / holder.offsetWidth;
      const cs = getComputedStyle(holder);
      const items = [...ul.querySelectorAll('li')].map((li) => li.textContent.trim());
      return {
        count: items.length, items, sample: items.slice(0, 6),
        rect: {x: box.x, y: box.y, w: box.width, h: box.height},
        borderWidth: parseFloat(cs.borderTopWidth) * scale,
        backgroundColor: cs.backgroundColor,
        backgroundIsOpaque: !/rgba\\(0, 0, 0, 0\\)|transparent/.test(cs.backgroundColor),
      };
    }"""
    )


@pytest.fixture(scope="module")
def live_key() -> str:
    key = os.environ.get("ORCAROUTER_API_KEY")
    if not key:
        pytest.skip("ORCAROUTER_API_KEY is not set")
    return key


@pytest.fixture(scope="module")
def panel_url(live_key):
    """Start the real Gradio app against the live catalog on a loopback port."""
    os.environ.setdefault("GRADIO_ANALYTICS_ENABLED", "False")
    os.environ.setdefault("CUA_TELEMETRY_ENABLED", "false")
    os.environ["ORCA_KEY"] = live_key
    os.environ["ORCA_KEY_SOURCE"] = "api_key"

    provider = install_orcarouter_provider()
    session = OrcaRouterConnectSession(provider=provider)
    demo = create_gradio_ui(provider, session)

    port = _free_port()
    demo.launch(
        server_name="127.0.0.1",
        server_port=port,
        prevent_thread_lock=True,
        show_error=True,
        quiet=True,
        ssr_mode=False,
        inbrowser=False,
    )
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                break
        except OSError:
            time.sleep(0.5)
    else:  # pragma: no cover - startup failure is an environment error
        raise RuntimeError("the Gradio app did not start listening")

    yield f"http://127.0.0.1:{port}/", provider
    demo.close()


class TestOrcaRouterGuiEvidence:
    def test_authentication_choices_and_catalog_dropdowns(
        self, panel_url, live_key, tmp_path_factory
    ):
        from playwright.sync_api import sync_playwright

        url, provider = panel_url
        evidence = _repository_root() / "orca-evidence"
        evidence.mkdir(exist_ok=True)

        # The dropdowns must be the live catalog, so the panel's own catalog
        # result is the reference the captures are compared against.
        live = provider.load_catalog(capability="chat", use_cache=False)
        assert live.origin == "live", f"catalog degraded: {live.degraded_reason}"
        assert live.models, "the live chat catalog returned no models"

        manifest: dict = {
            "automation": {
                "framework": "playwright",
                "tool": "python-playwright",
                "browser": CHROMIUM,
                "ui_entry_point": (
                    "libs/python/agent/cua_agent/ui/gradio/ui_components.py::create_gradio_ui"
                ),
                "url": url,
                "catalog_source": CATALOG_SOURCE,
                "passed": False,
                "checks": {},
            },
            "artifacts": [],
        }
        checks = manifest["automation"]["checks"]

        with sync_playwright() as pw:
            browser = pw.chromium.launch(executable_path=CHROMIUM, args=["--no-sandbox"])
            page = browser.new_page(viewport=VIEWPORT, device_scale_factor=1)
            page.goto(url, wait_until="domcontentloaded", timeout=60_000)
            page.wait_for_timeout(6000)

            # -- select the OrcaRouter loop ---------------------------------
            panel = page.locator("text=OrcaRouter - API").first
            if not panel.is_visible():
                page.locator("input[aria-label='Agent Loop']").click()
                page.wait_for_timeout(1000)
                page.locator("li[role='option']", has_text="ORCAROUTER").first.click()
                page.wait_for_timeout(12000)
            assert panel.is_visible(), "the OrcaRouter panel did not become visible"

            # -- auth-methods.png -------------------------------------------
            body = page.evaluate("document.body.innerText")
            api_field = page.locator("input[placeholder='sk-orca-...']")
            save_button = page.locator("button:has-text('Save key')")
            connect_button = page.locator("button:has-text('Connect with OrcaRouter')")
            assert api_field.count() == 1, "the API-key field is missing"
            assert connect_button.count() >= 1, "the PKCE connect button is missing"
            checks["auth_methods"] = {
                "api_key_visible": api_field.first.is_visible(),
                "pkce_visible": connect_button.first.is_visible(),
                # The real secret must never reach the page: only the masked form.
                "secret_masked": ("sk-orca-…redacted" in body) and (live_key not in body),
                "controls_enabled": (
                    save_button.first.is_enabled() and connect_button.first.is_enabled()
                ),
                "api_key_field_is_password": api_field.first.get_attribute("type") == "password",
            }
            assert live_key not in body, "the API key leaked into the rendered page"

            top = page.evaluate(
                """() => {
              const el = [...document.querySelectorAll('*')].find(
                (e) => e.textContent.trim().startsWith('OrcaRouter - API'));
              return el.getBoundingClientRect().top + window.scrollY;
            }"""
            )
            page.evaluate(f"window.scrollTo(0, {max(0, top - 120)})")
            page.wait_for_timeout(700)
            auth_shot = evidence / "auth-methods.png"
            page.screenshot(path=str(auth_shot))

            # -- text-model-dropdown.png ------------------------------------
            dropdown = page.locator("input[aria-label='OrcaRouter Model']").first
            dropdown.scroll_into_view_if_needed()
            page.wait_for_timeout(500)
            trigger = _trigger_box(page, "OrcaRouter Model")
            dropdown.click()
            page.wait_for_timeout(2500)
            popup = _open_option_list(page)
            assert popup is not None, "the text model dropdown did not open"
            # Options come from the live catalog, not a hand-written example list.
            assert popup["count"] == len(live.models), (
                f"dropdown shows {popup['count']} options but the live catalog returned "
                f"{len(live.models)}"
            )
            checks["text_model_dropdown"] = {
                "dropdown_open": popup["rect"]["h"] > 0,
                "item_count": popup["count"],
                "opaque_background": popup["backgroundIsOpaque"],
                "visible_border": popup["borderWidth"] >= 1.0,
                "trigger_panel_right_delta": round(
                    abs(
                        (trigger["x"] + trigger["width"])
                        - (popup["rect"]["x"] + popup["rect"]["w"])
                    ),
                    2,
                ),
                "vendor_namespace_preserved": all(
                    item.split("orcarouter/", 1)[1].count("/") >= 1 for item in popup["items"]
                ),
                "sample_items": popup["sample"],
            }
            text_shot = evidence / "text-model-dropdown.png"
            page.screenshot(path=str(text_shot))

            # -- multimodal-model-dropdown.png ------------------------------
            page.keyboard.press("Escape")
            page.wait_for_timeout(500)
            modality = page.locator(
                "label:has-text('Screenshot understanding (image input)') input[type='checkbox']"
            ).first
            modality.scroll_into_view_if_needed()
            modality.check()
            page.wait_for_timeout(6000)
            dropdown.scroll_into_view_if_needed()
            page.wait_for_timeout(500)
            trigger = _trigger_box(page, "OrcaRouter Model")
            dropdown.click()
            page.wait_for_timeout(2500)
            multimodal = _open_option_list(page)
            assert multimodal is not None, "the multimodal model dropdown did not open"

            image_input = [model.id for model in live.models if "image" in model.input_modalities]
            assert image_input, "no live chat model declares image input"
            assert multimodal["count"] == len(image_input), (
                f"multimodal dropdown shows {multimodal['count']} options but "
                f"{len(image_input)} live chat models declare image input"
            )
            # A text-only chat model must not survive the multimodal filter.
            assert all(
                item.split("orcarouter/", 1)[1].split("  [")[0] in image_input
                for item in multimodal["items"]
            ), "a text-only model leaked into the multimodal dropdown"
            checks["multimodal_model_dropdown"] = {
                "dropdown_open": multimodal["rect"]["h"] > 0,
                "item_count": multimodal["count"],
                "opaque_background": multimodal["backgroundIsOpaque"],
                "visible_border": multimodal["borderWidth"] >= 1.0,
                "trigger_panel_right_delta": round(
                    abs(
                        (trigger["x"] + trigger["width"])
                        - (multimodal["rect"]["x"] + multimodal["rect"]["w"])
                    ),
                    2,
                ),
                "text_only_models_excluded": True,
                "sample_items": multimodal["sample"],
            }
            multimodal_shot = evidence / "multimodal-model-dropdown.png"
            page.screenshot(path=str(multimodal_shot))

            browser.close()

        for kind, shot in (
            ("auth-methods", auth_shot),
            ("text-model-dropdown", text_shot),
            ("multimodal-model-dropdown", multimodal_shot),
        ):
            manifest["artifacts"].append(
                {
                    "kind": kind,
                    "path": shot.name,
                    "sha256": _sha256(shot),
                    "ui": checks[kind.replace("-", "_")],
                }
            )

        manifest["automation"]["catalog_model_count"] = checks["text_model_dropdown"]["item_count"]
        manifest["automation"]["image_model_count"] = checks["multimodal_model_dropdown"][
            "item_count"
        ]
        manifest["automation"]["multimodal_chat_model_count"] = manifest["automation"][
            "image_model_count"
        ]
        manifest["automation"]["passed"] = all(
            all(value is True for value in check.values() if isinstance(value, bool))
            for check in checks.values()
        )
        (evidence / "manifest.json").write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )

        # The manifest is what the delivery gate re-validates; never hand it a
        # screenshot it cannot re-hash, and never let the secret into it.
        assert manifest["automation"]["passed"] is True
        assert live_key not in json.dumps(manifest)
        for artifact in manifest["artifacts"]:
            assert (evidence / artifact["path"]).stat().st_size > 10_000
