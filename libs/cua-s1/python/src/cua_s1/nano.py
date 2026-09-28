"""Cua-S1-Nano: a from-scratch, ~855K-parameter option-attention scorer for
computer-use GUI decisions.

Given a screen state (either a rendered accessibility-tree excerpt per
element, or a screenshot crop per element) and a fixed, closed set of
candidate (element, action) options, Cua-S1-Nano scores every option in one
parallel forward pass and returns a probability distribution over each
element's own options. It is a separate checkpoint family from
`cua-s1-form-v0`: it is not a form-filling specialist, and it makes no use of
`cua_s1.planner`, `cua_s1.driver`, or `cua_s1.synth`, which remain specific to
the form-filling checkpoint.

Two context modalities are supported, selected per batch via
`batch["modality"]`:

- `"text"`: context tokens come from `NanoTextContextEncoder`, a small
  trainable byte-embedding + transformer encoder over a rendered
  accessibility-tree excerpt for the element. This path has no dependency on
  any vision backbone.
- `"multimodal"`: context tokens come from a frozen vision backbone's
  per-element screenshot-crop features, projected down by a small trainable
  `visual_proj` layer. The backbone is selected explicitly via the
  `vision_backbone` config field (`"smolvlm"` or `"siglip"`); this library
  never chooses a backbone from an environment variable, since that would be
  surprising behavior for public library code. Loading a vision backbone
  requires the optional `transformers` and `pillow` dependencies and
  downloads pretrained weights on first use; `NanoScorer` itself never
  imports them unless `encode_images` is actually called.

Options are always rendered as text and encoded by `NanoOptionEncoder`
regardless of modality. Both context encoders and the option encoder share
one hidden width, so a single `AttentionHead` (from `cua_s1.model`) serves
both modalities; only one of the two context encoders runs per forward call.

This module is inference- and scoring-only. It intentionally does not include
a training loop or the data-generation code used to produce a `cua-s1-nano`
checkpoint; see `libs/cua-s1/training/` for the package's existing training
entry points, and the module docstring there for the status of nano training
support.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import torch
from torch import nn

from .checkpoint import load_checkpoint_files, save_checkpoint_files
from .model import AttentionHead

VISION_FEATURE_DIM = 768  # hidden size shared by both supported vision backbones

TensorBatch = dict[str, torch.Tensor]


@dataclass(frozen=True)
class NanoElement:
    """One GUI element: its rendered context and its candidate options.

    `context` is either a rendered accessibility-tree excerpt (text
    modality) or ignored placeholder text (multimodal modality, where the
    context tokens instead come from a screenshot crop supplied out of
    band via `crop_features`). `options` are always rendered as text.
    """

    element_id: str
    context: str
    options: tuple[str, ...]


def _byte_ids(text: str, length: int) -> list[int]:
    return [byte + 1 for byte in text.encode("utf-8", errors="replace")[:length]]


class NanoByteCollator:
    """Collate `NanoElement`s into padded UTF-8 byte tensors for the text
    modality. Produces the same tensor shapes `NanoScorer.forward` expects
    under `batch["modality"] == "text"`."""

    def __init__(self, context_tokens: int, option_tokens: int) -> None:
        if context_tokens <= 0 or option_tokens <= 0:
            raise ValueError("token limits must be positive")
        self.context_tokens = context_tokens
        self.option_tokens = option_tokens

    def __call__(self, elements: Sequence[NanoElement]) -> TensorBatch:
        if not elements:
            raise ValueError("cannot collate an empty batch")
        contexts = [_byte_ids(item.context, self.context_tokens) for item in elements]
        option_rows = [
            [_byte_ids(option, self.option_tokens) for option in item.options] for item in elements
        ]
        batch = _tensor_batch(contexts, option_rows)
        batch["modality"] = "text"
        return batch


def _tensor_batch(
    contexts: Sequence[Sequence[int]],
    option_rows: Sequence[Sequence[Sequence[int]]],
) -> TensorBatch:
    batch = len(contexts)
    max_context = max(1, max(map(len, contexts)))
    max_options = max(len(row) for row in option_rows)
    max_option_tokens = max(
        1, max((len(tokens) for row in option_rows for tokens in row), default=1)
    )
    context_ids = torch.zeros((batch, max_context), dtype=torch.long)
    option_ids = torch.zeros((batch, max_options, max_option_tokens), dtype=torch.long)
    option_mask = torch.zeros((batch, max_options), dtype=torch.bool)
    for row, tokens in enumerate(contexts):
        if tokens:
            context_ids[row, : len(tokens)] = torch.tensor(tokens, dtype=torch.long)
    for row, options in enumerate(option_rows):
        option_mask[row, : len(options)] = True
        for column, tokens in enumerate(options):
            if tokens:
                option_ids[row, column, : len(tokens)] = torch.tensor(tokens, dtype=torch.long)
    return {
        "context_ids": context_ids,
        "context_token_mask": context_ids.ne(0),
        "option_ids": option_ids,
        "option_token_mask": option_ids.ne(0),
        "option_mask": option_mask,
    }


class NanoOptionEncoder(nn.Module):
    """Byte embeddings + a 1-layer transformer, mean-pooled per option."""

    def __init__(
        self, width: int, option_tokens: int, heads: int = 4, dropout: float = 0.1
    ) -> None:
        super().__init__()
        if width % heads:
            raise ValueError("model width must be divisible by attention heads")
        self.embedding = nn.Embedding(257, width, padding_idx=0)
        self.position = nn.Embedding(option_tokens, width)
        self.encoder = nn.TransformerEncoder(
            nn.TransformerEncoderLayer(
                width, heads, width * 4, dropout, batch_first=True, norm_first=True
            ),
            1,
        )

    def forward(self, option_ids: torch.Tensor, option_token_mask: torch.Tensor) -> torch.Tensor:
        b, n, t = option_ids.shape
        flat_ids = option_ids.reshape(b * n, t)
        flat_mask = option_token_mask.reshape(b * n, t)
        safe_mask = flat_mask.clone()
        safe_mask[:, 0] = True  # padded/absent options still need one valid key for the transformer
        positions = torch.arange(t, device=option_ids.device)
        embedded = self.embedding(flat_ids) + self.position(positions)
        hidden = self.encoder(embedded, src_key_padding_mask=~safe_mask)
        weights = flat_mask.unsqueeze(-1).float()
        pooled = (hidden * weights).sum(1) / weights.sum(1).clamp_min(1)
        return pooled.reshape(b, n, -1)


class NanoTextContextEncoder(nn.Module):
    """Small trainable byte transformer over an element's rendered context.

    Produces a full token sequence (not pooled) so the attention head has a
    real set of context tokens to attend over, matching the shape the
    multimodal path produces (a sequence of patch tokens, not one vector).
    """

    def __init__(
        self, width: int, context_tokens: int, heads: int = 4, layers: int = 2, dropout: float = 0.1
    ) -> None:
        super().__init__()
        if width % heads:
            raise ValueError("model width must be divisible by attention heads")
        self.embedding = nn.Embedding(257, width, padding_idx=0)
        self.position = nn.Embedding(context_tokens, width)
        self.encoder = nn.TransformerEncoder(
            nn.TransformerEncoderLayer(
                width, heads, width * 4, dropout, batch_first=True, norm_first=True
            ),
            layers,
        )

    def forward(self, context_ids: torch.Tensor, context_token_mask: torch.Tensor) -> torch.Tensor:
        safe_mask = context_token_mask.clone()
        safe_mask[:, 0] = True
        positions = torch.arange(context_ids.shape[1], device=context_ids.device)
        embedded = self.embedding(context_ids) + self.position(positions)
        return self.encoder(embedded, src_key_padding_mask=~safe_mask)


class NanoScorer(nn.Module):
    """The Cua-S1-Nano architecture over a closed option set.

    `forward` takes a collated batch whose `"modality"` key is either
    `"multimodal"` or `"text"` and picks the matching context encoder; both
    share one `AttentionHead` and one `NanoOptionEncoder`.
    """

    def __init__(
        self,
        width: int = 128,
        rank: int = 128,
        option_tokens: int = 96,
        context_tokens: int = 256,
        vision_dim: int = VISION_FEATURE_DIM,
    ) -> None:
        super().__init__()
        if min(width, rank, option_tokens, context_tokens, vision_dim) <= 0:
            raise ValueError("model dimensions must be positive")
        self.visual_proj = nn.Sequential(nn.LayerNorm(vision_dim), nn.Linear(vision_dim, width))
        self.text_context_encoder = NanoTextContextEncoder(width, context_tokens)
        self.option_encoder = NanoOptionEncoder(width, option_tokens)
        self.head = AttentionHead(width, rank)
        self.width, self.rank = width, rank
        self.option_tokens, self.context_tokens, self.vision_dim = (
            option_tokens,
            context_tokens,
            vision_dim,
        )

    def forward(self, batch: TensorBatch, shuffle_context: bool = False) -> torch.Tensor:
        modality = batch["modality"]
        if modality == "multimodal":
            context = self.visual_proj(batch["crop_features"])
            context_mask = batch["crop_mask"]
        elif modality == "text":
            context = self.text_context_encoder(batch["context_ids"], batch["context_token_mask"])
            context_mask = batch["context_token_mask"]
        else:
            raise ValueError(f"unknown modality: {modality!r}")
        options = self.option_encoder(batch["option_ids"], batch["option_token_mask"])
        return self.head(context, context_mask, options, batch["option_mask"], shuffle_context)

    @torch.no_grad()
    def score_elements(
        self,
        elements: Sequence[NanoElement],
        collator: NanoByteCollator,
        device: torch.device | None = None,
    ) -> dict[str, dict[int, float]]:
        """Score a batch of text-modality elements against their own options.

        Returns, per `element_id`, a dict mapping the index of each of that
        element's candidate options (in `elements[i].options` order) to a
        softmax probability. Each element's own distribution sums to ~1 and
        covers exactly its offered options.
        """
        if not elements:
            return {}
        device = device or next(self.parameters()).device
        batch = collator(elements)
        batch = {k: (v.to(device) if isinstance(v, torch.Tensor) else v) for k, v in batch.items()}
        logits = self.forward(batch)
        probs = torch.softmax(logits, dim=-1)
        result: dict[str, dict[int, float]] = {}
        for i, element in enumerate(elements):
            n = len(element.options)
            result[element.element_id] = {j: float(probs[i, j]) for j in range(n)}
        return result


def select_device(name: str = "auto") -> torch.device:
    """Resolve an explicit device or select the best available local device."""
    if name != "auto":
        return torch.device(name)
    if torch.cuda.is_available():
        return torch.device("cuda")
    if torch.backends.mps.is_available():
        return torch.device("mps")
    return torch.device("cpu")


def _positive_int(config: Mapping[str, Any], key: str, default: int | None = None) -> int:
    value = config.get(key, default)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"model config field {key!r} must be a positive integer")
    return value


def make_nano_system(
    config: Mapping[str, Any], device: torch.device | str
) -> tuple[NanoScorer, NanoByteCollator]:
    """Construct a `NanoScorer` and matching text collator from validated
    configuration. `config["vision_backbone"]` (if present) must be
    `"smolvlm"` or `"siglip"`; it is not read or used here (multimodal
    context is supplied by the caller as pre-computed `crop_features`), but
    it is validated so a bad config fails fast rather than silently loading
    the wrong backbone later via `load_vision_backbone`.
    """
    target = select_device(device) if isinstance(device, str) else device
    backbone = config.get("vision_backbone")
    if backbone is not None and backbone not in ("smolvlm", "siglip"):
        raise ValueError("model config field 'vision_backbone' must be 'smolvlm' or 'siglip'")
    model = NanoScorer(
        width=_positive_int(config, "width", 128),
        rank=_positive_int(config, "rank", 128),
        option_tokens=_positive_int(config, "option_tokens", 96),
        context_tokens=_positive_int(config, "context_tokens", 256),
        vision_dim=_positive_int(config, "vision_dim", VISION_FEATURE_DIM),
    )
    collator = NanoByteCollator(model.context_tokens, model.option_tokens)
    return model.to(target), collator


def trainable_state(model: nn.Module) -> dict[str, torch.Tensor]:
    """Copy trainable parameters to contiguous CPU tensors."""
    return {
        name: parameter.detach().cpu().contiguous()
        for name, parameter in model.named_parameters()
        if parameter.requires_grad
    }


def save_nano_checkpoint(
    directory_or_path: str | Path,
    model: NanoScorer,
    config: Mapping[str, Any],
    metadata: Mapping[str, Any] | None = None,
) -> tuple[Path, Path]:
    """Save a `NanoScorer`'s trainable state as safetensors with a JSON
    configuration, reusing `cua_s1.checkpoint`'s format-agnostic,
    safetensors-only save path unmodified."""
    return save_checkpoint_files(directory_or_path, trainable_state(model), config, metadata)


def load_nano_checkpoint(
    directory_or_path: str | Path, device: torch.device | str
) -> tuple[NanoScorer, NanoByteCollator, dict[str, Any]]:
    """Load a `NanoScorer` from safetensors and JSON without executing
    serialized code, reusing `cua_s1.checkpoint`'s format-agnostic,
    safetensors-only load path unmodified."""
    state_dict, config, _metadata = load_checkpoint_files(directory_or_path)
    model, collator = make_nano_system(config, device)
    try:
        missing, unexpected = model.load_state_dict(state_dict, strict=False)
    except RuntimeError as exc:
        raise ValueError(f"checkpoint tensor mismatch: {exc}") from exc
    if missing or unexpected:
        raise ValueError(
            f"checkpoint mismatch: missing={list(missing)}, unexpected={list(unexpected)}"
        )
    model.eval()
    return model, collator, config


def parameter_count(model: nn.Module) -> int:
    """Return the number of trainable scalar parameters."""
    return sum(parameter.numel() for parameter in model.parameters() if parameter.requires_grad)


# ---------------------------------------------------------------------------
# Optional frozen vision backbones for the multimodal modality.
#
# Neither backbone is imported, downloaded, or instantiated unless
# `load_vision_backbone` is actually called; `NanoScorer` and the rest of
# this module have no import-time dependency on `transformers` or `PIL`.
# Install the `nano-vision` extra to use either backbone.
# ---------------------------------------------------------------------------


class SmolVlmVisionBackbone:
    """Frozen SmolVLM-256M-Instruct vision tower (a SigLIP-style
    Idefics3VisionTransformer, ~86.4M parameters). Only used to produce
    per-element screenshot-crop features for the multimodal modality; the
    backbone itself is never trained."""

    MODEL_NAME = "HuggingFaceTB/SmolVLM-256M-Instruct"
    FEATURE_DIM = VISION_FEATURE_DIM
    RAW_GRID = 32  # 512x512 processed image / patch_size 16 -> 32x32 raw patches
    POOLED_GRID = 8  # adaptive-pooled down to 8x8 so per-crop feature sets stay small
    TOKENS_PER_CROP = POOLED_GRID * POOLED_GRID

    def __init__(self, device: str = "auto", dtype: "torch.dtype" = torch.float16) -> None:
        from transformers import AutoProcessor, Idefics3ForConditionalGeneration

        resolved = select_device(device)
        self.device = resolved
        self.dtype = dtype if self.device.type == "cuda" else torch.float32
        processor = AutoProcessor.from_pretrained(self.MODEL_NAME)
        self.image_processor = processor.image_processor
        self.patch_size = 16
        model = Idefics3ForConditionalGeneration.from_pretrained(
            self.MODEL_NAME, dtype=self.dtype, device_map=str(self.device)
        )
        self.vision_model = model.model.vision_model.eval()
        for parameter in self.vision_model.parameters():
            parameter.requires_grad_(False)
        del model
        if self.device.type == "cuda":
            torch.cuda.empty_cache()

    def _patch_attention_mask(self, pixel_attention_mask: torch.Tensor) -> torch.Tensor:
        p = self.patch_size
        subgrid = pixel_attention_mask.unfold(1, p, p).unfold(2, p, p)
        return (subgrid.sum(dim=(-1, -2)) > 0).bool()

    @torch.no_grad()
    def encode(self, images: Sequence[Any], batch_size: int = 64) -> torch.Tensor:
        """Returns `(N, TOKENS_PER_CROP, FEATURE_DIM)` float32 CPU tensor,
        one row per image, adaptive-pooled down to `POOLED_GRID`x`POOLED_GRID`
        patches per crop."""
        all_features = []
        for start in range(0, len(images), batch_size):
            batch = list(images[start : start + batch_size])
            out = self.image_processor(images=batch, return_tensors="pt", do_image_splitting=False)
            pixel_values = out["pixel_values"].to(self.device, dtype=self.dtype)
            b, n, c, h, w = pixel_values.shape
            pixel_values = pixel_values.reshape(b * n, c, h, w)
            pixel_attention_mask = out["pixel_attention_mask"].to(self.device)
            pixel_attention_mask = pixel_attention_mask.reshape(
                b * n, pixel_attention_mask.shape[-2], pixel_attention_mask.shape[-1]
            )
            patch_mask = self._patch_attention_mask(pixel_attention_mask)
            features = self.vision_model(
                pixel_values=pixel_values, patch_attention_mask=patch_mask
            ).last_hidden_state
            features = self._pool(features)
            all_features.append(features.float().cpu())
        return torch.cat(all_features, dim=0)

    def _pool(self, features: torch.Tensor) -> torch.Tensor:
        b = features.shape[0]
        grid = features.reshape(b, self.RAW_GRID, self.RAW_GRID, self.FEATURE_DIM).permute(
            0, 3, 1, 2
        )
        pooled = torch.nn.functional.adaptive_avg_pool2d(grid, (self.POOLED_GRID, self.POOLED_GRID))
        return pooled.permute(0, 2, 3, 1).reshape(
            b, self.POOLED_GRID * self.POOLED_GRID, self.FEATURE_DIM
        )


class SiglipVisionBackbone:
    """Alternative frozen backbone: `google/siglip-base-patch16-224`
    (~93M parameters). 224x224 input with patch size 16 gives 14x14=196 raw
    patches; hidden size 768 matches `VISION_FEATURE_DIM` exactly, so
    `NanoScorer` needs no changes to use either backbone's output -- only the
    per-crop feature tensors differ."""

    MODEL_NAME = "google/siglip-base-patch16-224"
    FEATURE_DIM = VISION_FEATURE_DIM
    RAW_GRID = 14
    POOLED_GRID = 8
    TOKENS_PER_CROP = POOLED_GRID * POOLED_GRID

    def __init__(self, device: str = "auto", dtype: "torch.dtype" = torch.float16) -> None:
        from transformers import AutoImageProcessor, SiglipVisionModel

        resolved = select_device(device)
        self.device = resolved
        self.dtype = dtype if self.device.type == "cuda" else torch.float32
        self.image_processor = AutoImageProcessor.from_pretrained(self.MODEL_NAME)
        self.vision_model = (
            SiglipVisionModel.from_pretrained(self.MODEL_NAME, dtype=self.dtype)
            .to(self.device)
            .eval()
        )
        for parameter in self.vision_model.parameters():
            parameter.requires_grad_(False)
        if self.vision_model.config.hidden_size != self.FEATURE_DIM:
            raise ValueError(
                f"siglip hidden_size {self.vision_model.config.hidden_size} != "
                f"expected {self.FEATURE_DIM}"
            )
        if self.device.type == "cuda":
            torch.cuda.empty_cache()

    @torch.no_grad()
    def encode(self, images: Sequence[Any], batch_size: int = 64) -> torch.Tensor:
        all_features = []
        for start in range(0, len(images), batch_size):
            batch = list(images[start : start + batch_size])
            out = self.image_processor(images=batch, return_tensors="pt")
            pixel_values = out["pixel_values"].to(self.device, dtype=self.dtype)
            features = self.vision_model(pixel_values=pixel_values).last_hidden_state
            features = self._pool(features)
            all_features.append(features.float().cpu())
        return torch.cat(all_features, dim=0)

    def _pool(self, features: torch.Tensor) -> torch.Tensor:
        b = features.shape[0]
        grid = features.reshape(b, self.RAW_GRID, self.RAW_GRID, self.FEATURE_DIM).permute(
            0, 3, 1, 2
        )
        pooled = torch.nn.functional.adaptive_avg_pool2d(grid, (self.POOLED_GRID, self.POOLED_GRID))
        return pooled.permute(0, 2, 3, 1).reshape(
            b, self.POOLED_GRID * self.POOLED_GRID, self.FEATURE_DIM
        )


def load_vision_backbone(
    name: str, device: str = "auto"
) -> SmolVlmVisionBackbone | SiglipVisionBackbone:
    """Explicitly construct one of the two supported frozen vision backbones.

    Unlike the research harness this architecture was ported from,
    `name` is a required, explicit argument here -- never an environment
    variable -- since implicit environment-driven backbone selection is
    surprising behavior for public library code. Requires the optional
    `transformers` and `pillow` dependencies (the `nano-vision` extra) and
    downloads pretrained weights on first use.
    """
    if name == "smolvlm":
        return SmolVlmVisionBackbone(device)
    if name == "siglip":
        return SiglipVisionBackbone(device)
    raise ValueError("vision backbone name must be 'smolvlm' or 'siglip'")
