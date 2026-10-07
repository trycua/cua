"""Compile the product geometry guard without a running Wayland desktop."""
import ast
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest


class CaptureGeometryGuard(unittest.TestCase):
    def test_resize_aba_invalidates_but_content_only_commits_do_not(self):
        patch = Path(__file__).resolve().parents[1] / "cua_compositor_patch.py"
        module = ast.parse(patch.read_text())
        functions = next(
            ast.literal_eval(node.value)
            for node in module.body
            if isinstance(node, ast.Assign)
            and any(isinstance(target, ast.Name) and target.id == "FUNCS" for target in node.targets)
        )
        start = functions.index("static void cua_capture_geometry_committed(")
        end = functions.index("\nstatic void cua_capture_title_changed(", start)
        guard = functions[start:end]
        source = r"""
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
struct wlr_box { int x, y, width, height; };
struct surface { struct wlr_box geometry; };
struct toplevel { struct surface *base; };
struct tinywl_toplevel {
    struct toplevel *xdg_toplevel;
    struct wlr_box capture_geometry;
    bool capture_geometry_known;
};
static uint64_t g_capture_epoch = 1;
""" + guard + r"""
int main(void) {
    struct surface surface = { .geometry = { 0, 0, 640, 480 } };
    struct toplevel xdg = { .base = &surface };
    struct tinywl_toplevel target = { .xdg_toplevel = &xdg };
    cua_capture_geometry_committed(&target);
    uint64_t before = g_capture_epoch;
    cua_capture_geometry_committed(&target);
    assert(g_capture_epoch == before);
    surface.geometry.width = 320;
    cua_capture_geometry_committed(&target);
    surface.geometry.width = 640;
    cua_capture_geometry_committed(&target);
    assert(g_capture_epoch == before + 2);
    assert(target.capture_geometry.width == 640);
    before = g_capture_epoch;
    surface.geometry.x = 10;
    cua_capture_geometry_committed(&target);
    assert(g_capture_epoch == before + 1);
    cua_capture_geometry_committed(&target);
    assert(g_capture_epoch == before + 1);
    return 0;
}
"""
        with tempfile.TemporaryDirectory() as directory:
            code = Path(directory) / "guard.c"
            binary = Path(directory) / "guard"
            code.write_text(source)
            compiler = shlex.split(os.environ.get("CC", "cc"))
            subprocess.run(compiler + ["-std=c11", "-Wall", "-Wextra", "-Werror", str(code), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
