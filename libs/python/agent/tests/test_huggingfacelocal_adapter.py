"""Unit tests for HuggingFaceLocalAdapter.

This file tests ONLY the HuggingFaceLocalAdapter class without requiring
torch, transformers, or any GPU hardware to be present.

All heavy dependencies (torch, transformers, AutoConfig, AutoProcessor,
AutoModelForImageTextToText) are mocked at the module level so that the
test suite runs safely on Windows/Anaconda environments where the
PyTorch/OMP duplicate-library crash (issue #1381) would otherwise abort
the process.

Fixes: https://github.com/trycua/cua/issues/1381
"""

import sys
from types import ModuleType
from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest

# ---------------------------------------------------------------------------
# Module-level mocking: inject fake `torch` and `transformers` BEFORE the
# adapter module is imported.  This prevents the real torch from being
# loaded (and triggering the OMP duplicate-library crash on Windows).
# ---------------------------------------------------------------------------

_fake_torch = ModuleType("torch")
_fake_torch.cuda = MagicMock()  # type: ignore[attr-defined]
_fake_torch.cuda.is_available = Mock(return_value=False)

_fake_transformers = ModuleType("transformers")
_fake_transformers.AutoModelForImageTextToText = MagicMock()  # type: ignore[attr-defined]
_fake_transformers.AutoProcessor = MagicMock()  # type: ignore[attr-defined]
_fake_transformers.AutoConfig = MagicMock()  # type: ignore[attr-defined]

# Patch into sys.modules so any `import torch` / `import transformers` in
# the adapter resolves to our fakes.
sys.modules.setdefault("torch", _fake_torch)
sys.modules.setdefault("transformers", _fake_transformers)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_adapter(**kwargs):
    """Import and instantiate HuggingFaceLocalAdapter with mocked deps."""
    # Patch load_model so no real model weights are loaded.
    with patch("cua_agent.adapters.huggingfacelocal_adapter.load_model_handler") as _:
        from cua_agent.adapters.huggingfacelocal_adapter import HuggingFaceLocalAdapter

        return HuggingFaceLocalAdapter(**kwargs)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestHuggingFaceLocalAdapterInit:
    """Test adapter initialisation (SRP: only tests __init__)."""

    def test_default_init(self):
        """Adapter can be instantiated with default arguments."""
        adapter = _make_adapter()
        assert adapter.device == "auto"
        assert adapter.trust_remote_code is False
        assert adapter._handlers == {}

    def test_custom_device(self):
        """Adapter stores a custom device string."""
        adapter = _make_adapter(device="cuda")
        assert adapter.device == "cuda"

    def test_trust_remote_code_flag(self):
        """Adapter stores trust_remote_code flag correctly."""
        adapter = _make_adapter(trust_remote_code=True)
        assert adapter.trust_remote_code is True


class TestConvertMessages:
    """Test _convert_messages (SRP: only tests message conversion)."""

    def setup_method(self):
        self.adapter = _make_adapter()

    def test_text_string_content(self):
        """Plain string content is wrapped in a text block."""
        messages = [{"role": "user", "content": "Hello"}]
        result = self.adapter._convert_messages(messages)
        assert result == [{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]

    def test_multimodal_text_item(self):
        """Text items in a list are preserved."""
        messages = [{"role": "user", "content": [{"type": "text", "text": "Hi"}]}]
        result = self.adapter._convert_messages(messages)
        assert result[0]["content"] == [{"type": "text", "text": "Hi"}]

    def test_image_url_converted_to_image(self):
        """image_url items are converted to image format."""
        url = "data:image/png;base64,abc123"
        messages = [
            {
                "role": "user",
                "content": [{"type": "image_url", "image_url": {"url": url}}],
            }
        ]
        result = self.adapter._convert_messages(messages)
        assert result[0]["content"] == [{"type": "image", "image": url}]

    def test_mixed_content(self):
        """Mixed text + image_url content is converted correctly."""
        url = "data:image/png;base64,xyz"
        messages = [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Describe this"},
                    {"type": "image_url", "image_url": {"url": url}},
                ],
            }
        ]
        result = self.adapter._convert_messages(messages)
        assert result[0]["content"] == [
            {"type": "text", "text": "Describe this"},
            {"type": "image", "image": url},
        ]

    def test_multiple_messages(self):
        """Multiple messages are all converted."""
        messages = [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi there"},
        ]
        result = self.adapter._convert_messages(messages)
        assert len(result) == 2
        assert result[0]["role"] == "user"
        assert result[1]["role"] == "assistant"

    def test_empty_messages(self):
        """Empty list returns empty list."""
        assert self.adapter._convert_messages([]) == []


class TestGenerateWithoutHFDeps:
    """Test _generate when HF dependencies are unavailable (SRP: import guard)."""

    def test_raises_import_error_when_hf_unavailable(self):
        """_generate raises ImportError with helpful message when HF deps are missing."""
        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", False):
            adapter = _make_adapter()
            with pytest.raises(ImportError, match="cua-agent\\[uitars-hf\\]"):
                adapter._generate(
                    messages=[{"role": "user", "content": "hello"}],
                    model="ByteDance-Seed/UI-TARS-1.5-7B",
                )


class TestGenerateWithMockedHF:
    """Test _generate when HF dependencies ARE available (mocked)."""

    def setup_method(self):
        self.adapter = _make_adapter()

    def _mock_handler(self, return_text: str = "click(100, 200)") -> Mock:
        handler = Mock()
        handler.generate = Mock(return_value=return_text)
        return handler

    def test_generate_delegates_to_handler(self):
        """_generate calls handler.generate and returns its output."""
        handler = self._mock_handler("click(50, 80)")
        self.adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = handler

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            result = self.adapter._generate(
                messages=[{"role": "user", "content": "click something"}],
                model="ByteDance-Seed/UI-TARS-1.5-7B",
                max_tokens=64,
            )

        assert result == "click(50, 80)"
        handler.generate.assert_called_once()

    def test_generate_uses_default_max_tokens(self):
        """_generate passes max_tokens=128 when not specified."""
        handler = self._mock_handler()
        self.adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = handler

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            self.adapter._generate(
                messages=[{"role": "user", "content": "hi"}],
                model="ByteDance-Seed/UI-TARS-1.5-7B",
            )

        _, kwargs = handler.generate.call_args
        assert kwargs.get("max_new_tokens", 128) == 128

    def test_generate_warns_on_unknown_kwargs(self):
        """_generate emits a warning for unsupported kwargs."""
        handler = self._mock_handler()
        self.adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = handler

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            with pytest.warns(UserWarning, match="unsupported kwargs"):
                self.adapter._generate(
                    messages=[{"role": "user", "content": "hi"}],
                    model="ByteDance-Seed/UI-TARS-1.5-7B",
                    temperature=0.7,  # unsupported
                )

    def test_handler_is_cached(self):
        """Calling _get_handler twice for the same model reuses the same instance."""
        mock_load = Mock(return_value=Mock())
        with patch("cua_agent.adapters.huggingfacelocal_adapter.load_model_handler", mock_load):
            with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
                h1 = self.adapter._get_handler("some/model")
                h2 = self.adapter._get_handler("some/model")

        assert h1 is h2
        mock_load.assert_called_once()


class TestCompletionMethod:
    """Test synchronous completion() method."""

    def test_completion_returns_model_response(self):
        """completion() returns a ModelResponse-like object."""
        adapter = _make_adapter()
        adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = Mock(
            generate=Mock(return_value="action()")
        )

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            with patch("cua_agent.adapters.huggingfacelocal_adapter.completion") as mock_completion:
                mock_completion.return_value = {"choices": [{"message": {"content": "action()"}}]}
                result = adapter.completion(
                    messages=[{"role": "user", "content": "hi"}],
                    model="ByteDance-Seed/UI-TARS-1.5-7B",
                )

        mock_completion.assert_called_once()
        # model string should be prefixed
        call_kwargs = mock_completion.call_args[1]
        assert "huggingface-local/" in call_kwargs.get("model", "")


class TestAsyncCompletionMethod:
    """Test asynchronous acompletion() method."""

    @pytest.mark.asyncio
    async def test_acompletion_returns_model_response(self):
        """acompletion() returns a ModelResponse-like object."""
        adapter = _make_adapter()
        adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = Mock(
            generate=Mock(return_value="action()")
        )

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            with patch(
                "cua_agent.adapters.huggingfacelocal_adapter.acompletion",
                new_callable=AsyncMock,
            ) as mock_acompletion:
                mock_acompletion.return_value = {"choices": [{"message": {"content": "action()"}}]}
                result = await adapter.acompletion(
                    messages=[{"role": "user", "content": "hi"}],
                    model="ByteDance-Seed/UI-TARS-1.5-7B",
                )

        mock_acompletion.assert_called_once()
        call_kwargs = mock_acompletion.call_args[1]
        assert "huggingface-local/" in call_kwargs.get("model", "")


class TestStreamingMethod:
    """Test streaming() and astreaming() methods."""

    def test_streaming_yields_one_chunk(self):
        """streaming() yields exactly one GenericStreamingChunk."""
        adapter = _make_adapter()
        adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = Mock(
            generate=Mock(return_value="type('hello')")
        )

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            chunks = list(
                adapter.streaming(
                    messages=[{"role": "user", "content": "hi"}],
                    model="ByteDance-Seed/UI-TARS-1.5-7B",
                )
            )

        assert len(chunks) == 1
        assert chunks[0]["text"] == "type('hello')"
        assert chunks[0]["finish_reason"] == "stop"
        assert chunks[0]["is_finished"] is True

    @pytest.mark.asyncio
    async def test_astreaming_yields_one_chunk(self):
        """astreaming() yields exactly one GenericStreamingChunk."""
        adapter = _make_adapter()
        adapter._handlers["ByteDance-Seed/UI-TARS-1.5-7B"] = Mock(
            generate=Mock(return_value="type('world')")
        )

        with patch("cua_agent.adapters.huggingfacelocal_adapter.HF_AVAILABLE", True):
            chunks = []
            async for chunk in adapter.astreaming(
                messages=[{"role": "user", "content": "hi"}],
                model="ByteDance-Seed/UI-TARS-1.5-7B",
            ):
                chunks.append(chunk)

        assert len(chunks) == 1
        assert chunks[0]["text"] == "type('world')"
        assert chunks[0]["finish_reason"] == "stop"
