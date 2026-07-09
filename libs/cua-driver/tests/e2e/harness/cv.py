"""Computer-vision helpers for screenshot-based assertions.

All operations use PIL (Pillow) + numpy only — no OpenCV dependency.

Functions:
  decode(b64_str)                → PIL.Image
  crop(img, bbox)                → PIL.Image  (bbox: (x, y, w, h))
  diff_ratio(img_a, img_b)       → float in [0, 1]
  find_template(haystack, needle, threshold) → (x, y) or None  (NCC)
  save_reference(img, path)      → None
  load_reference(path)           → PIL.Image
"""

from __future__ import annotations

import base64
import io
import os
from typing import Optional, Tuple

try:
    from PIL import Image, ImageChops
    import numpy as np
    _PIL_AVAILABLE = True
except ImportError:
    _PIL_AVAILABLE = False


BBox = Tuple[int, int, int, int]  # x, y, w, h


def decode(b64_str: str) -> "Image.Image":
    """Decode a base64 PNG/JPEG string into a PIL Image."""
    if not _PIL_AVAILABLE:
        raise ImportError("Pillow is required for cv helpers: pip install pillow")
    data = base64.b64decode(b64_str)
    return Image.open(io.BytesIO(data)).convert("RGB")


def crop(img: "Image.Image", bbox: BBox) -> "Image.Image":
    """Crop a PIL image using (x, y, w, h) bounding box."""
    x, y, w, h = bbox
    return img.crop((x, y, x + w, y + h))


def diff_ratio(img_a: "Image.Image", img_b: "Image.Image") -> float:
    """Return fraction of pixels that differ between two same-size images.

    Returns a value in [0, 1]. 0 = identical, 1 = completely different.
    """
    if not _PIL_AVAILABLE:
        raise ImportError("Pillow is required for cv helpers")
    import numpy as np  # noqa: F811

    a = np.array(img_a.resize(img_b.size), dtype=np.float32)
    b = np.array(img_b, dtype=np.float32)
    diff = np.abs(a - b)
    changed = np.any(diff > 8, axis=-1)  # threshold per-channel
    return float(changed.sum()) / float(changed.size)


def find_template(
    haystack: "Image.Image",
    needle: "Image.Image",
    threshold: float = 0.85,
) -> Optional[Tuple[int, int]]:
    """Find `needle` inside `haystack` using normalised cross-correlation (NCC).

    Returns (x, y) top-left of the best match if similarity >= threshold,
    else None. No OpenCV required — uses numpy FFT-based NCC.
    """
    if not _PIL_AVAILABLE:
        raise ImportError("Pillow + numpy required for find_template")
    import numpy as np

    h_arr = np.array(haystack.convert("L"), dtype=np.float32)
    n_arr = np.array(needle.convert("L"), dtype=np.float32)

    H, W = h_arr.shape
    th, tw = n_arr.shape

    if th > H or tw > W:
        return None

    # Normalise needle
    n_mean = n_arr.mean()
    n_std = n_arr.std()
    if n_std < 1e-6:
        return None
    n_norm = (n_arr - n_mean) / n_std

    # Sliding-window NCC via FFT
    from numpy.fft import fft2, ifft2, fftshift

    # Zero-pad haystack to same size for FFT correlation
    pad_h = H + th - 1
    pad_w = W + tw - 1

    H_fft = fft2(h_arr, s=(pad_h, pad_w))
    N_fft = fft2(n_norm[::-1, ::-1], s=(pad_h, pad_w))
    corr = np.real(ifft2(H_fft * N_fft))

    # Local haystack std for normalisation
    from numpy.lib.stride_tricks import sliding_window_view
    windows = sliding_window_view(h_arr, (th, tw))
    local_std = windows.reshape(-1, th * tw).std(axis=1).reshape(H - th + 1, W - tw + 1)
    local_std[local_std < 1e-6] = 1.0

    # Trim correlation to valid region
    corr_valid = corr[th - 1:th - 1 + (H - th + 1), tw - 1:tw - 1 + (W - tw + 1)]
    ncc = corr_valid / (local_std * th * tw)

    best_idx = np.argmax(ncc)
    best_score = ncc.flat[best_idx]
    if best_score < threshold:
        return None

    ry, rx = divmod(int(best_idx), W - tw + 1)
    return (rx, ry)


def save_reference(img: "Image.Image", path: str) -> None:
    """Save a PIL Image as a PNG reference for later template matching."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.save(path, "PNG")


def load_reference(path: str) -> "Image.Image":
    """Load a reference PNG for template matching."""
    if not _PIL_AVAILABLE:
        raise ImportError("Pillow required")
    return Image.open(path).convert("RGB")
