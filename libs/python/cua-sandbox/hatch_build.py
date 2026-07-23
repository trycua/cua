import platform
import subprocess
from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


class CustomBuildHook(BuildHookInterface):
    repository_root = Path(__file__).resolve().parents[3]
    native_target_dir = repository_root / "target" / "cyclops-sdk-bindings-native"

    def initialize(self, version, build_data):
        mirror_root = self.repository_root / "libs" / "fleet"
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--manifest-path",
                str(mirror_root / "Cargo.toml"),
                "--package",
                "cyclops-sdk",
                "--target-dir",
                str(self.native_target_dir),
            ],
            check=True,
        )

        suffix = {"Darwin": ".dylib", "Linux": ".so"}.get(platform.system())
        if suffix is None:
            raise RuntimeError(f"unsupported host for Cyclops SDK bindings: {platform.system()}")

        native_library = self.native_target_dir / "debug" / f"libcyclops_sdk{suffix}"
        if not native_library.is_file():
            raise RuntimeError(f"native Cyclops SDK library was not built: {native_library}")

        build_data["infer_tag"] = True
        force_include = build_data.setdefault("force_include", {})
        force_include[str(mirror_root / "sdk-bindings" / "python" / "cyclops_sdk")] = "cyclops_sdk"
        force_include[str(native_library)] = f"cyclops_sdk/{native_library.name}"
