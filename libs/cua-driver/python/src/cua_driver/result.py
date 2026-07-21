"""Transport-neutral normalization of MCP tool results."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping


@dataclass(frozen=True)
class ImageContent:
    mime_type: str
    data_base64: str


@dataclass(frozen=True)
class ToolResult:
    text: str
    images: tuple[ImageContent, ...]
    structured: Mapping[str, Any] | None
    is_error: bool
    error_code: str | None
    verified: bool | None
    degraded: bool
    raw: Mapping[str, Any]

    @classmethod
    def from_mcp(cls, value: Mapping[str, Any]) -> "ToolResult":
        content = value.get("content")
        items = content if isinstance(content, list) else []
        text_parts: list[str] = []
        images: list[ImageContent] = []
        for item in items:
            if not isinstance(item, Mapping):
                continue
            if item.get("type") == "text" and isinstance(item.get("text"), str):
                text_parts.append(item["text"])
            elif (
                item.get("type") == "image"
                and isinstance(item.get("data"), str)
                and isinstance(item.get("mimeType"), str)
            ):
                images.append(ImageContent(item["mimeType"], item["data"]))

        raw_structured = value.get("structuredContent")
        structured = raw_structured if isinstance(raw_structured, Mapping) else None
        refusal = structured.get("refusal") if structured else None
        error_code = None
        if structured and isinstance(structured.get("code"), str):
            error_code = structured["code"]
        elif isinstance(refusal, Mapping) and isinstance(refusal.get("code"), str):
            error_code = refusal["code"]
        verified = structured.get("verified") if structured else None
        if not isinstance(verified, bool):
            verified = None
        degraded = bool(structured.get("degraded", False)) if structured else False

        return cls(
            text="\n".join(text_parts),
            images=tuple(images),
            structured=structured,
            is_error=value.get("isError") is True,
            error_code=error_code,
            verified=verified,
            degraded=degraded,
            raw=value,
        )
