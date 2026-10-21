"""PNG sidecars must be reachable from the saved trace's directory."""

from io import BytesIO
from pathlib import Path

import pytest
from cua_bench.tracing import Tracing
from datasets import load_from_disk
from PIL import Image


@pytest.mark.parametrize("layout", ["default", "nested", "sibling", "relative"])
@pytest.mark.parametrize("image_type", ["pil", "bytes", "bytearray"])
def test_png_sidecar_reference_resolves_to_saved_pixels(tmp_path, monkeypatch, layout, image_type):
    monkeypatch.chdir(tmp_path)
    output = tmp_path / "trace"
    image_dir = {
        "default": None,
        "nested": str(output / "assets" / "screenshots"),
        "sibling": str(tmp_path / "shared" / "screenshots"),
        "relative": "shared/screenshots",
    }[layout]
    image = Image.new("RGBA", (3, 2), (10, 20, 30, 255))
    source = image
    if image_type != "pil":
        png = BytesIO()
        image.save(png, format="PNG")
        source = png.getvalue() if image_type == "bytes" else bytearray(png.getvalue())
    trace = Tracing(env=None)
    trace.start("sidecar-paths")
    trace.record("reset", {"step": 0}, [source])
    trace.save_to_disk(str(output), save_pngs=True, image_dir=image_dir)

    saved = load_from_disk(str(output))
    references = saved[0]["data_images"]
    assert len(references) == 1
    reference = Path(references[0])
    assert not reference.is_absolute()
    expected_dir = Path(image_dir).resolve() if image_dir else output / "imgs"
    sidecar = (output / reference).resolve()
    assert sidecar.parent == expected_dir
    with Image.open(sidecar) as restored:
        assert restored.size == image.size
        assert restored.convert("RGBA").tobytes() == image.tobytes()
    if layout == "default":
        assert references[0].startswith("imgs/")
