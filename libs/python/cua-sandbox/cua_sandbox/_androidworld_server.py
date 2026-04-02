"""Configurable AndroidWorld FastAPI server launcher.

This is cua-sandbox's entry point for the AndroidWorld server. It reads
configuration from environment variables so that AndroidEmulatorRuntime can
start it as a subprocess with the right ADB serial, gRPC port, and setup flags.

Environment variables
---------------------
AW_CONSOLE_PORT     Android emulator console port (default: 5554)
AW_GRPC_PORT        emulator gRPC port (default: AW_CONSOLE_PORT + 3000)
AW_ADB_PATH         path to adb binary (default: auto-detect from SDK)
AW_EMULATOR_SETUP   "true" to run full app install (default: "false")
                    Set to "true" only when baking a fresh AVD.
AW_FREEZE_DATETIME  "true" to freeze device clock to Oct 2023 (default: "true")
AW_PORT             port for this FastAPI server to listen on (default: 5000)
AW_HOST             bind host (default: 0.0.0.0)
"""

from __future__ import annotations

import contextlib
import os
import typing
from typing import Any

import fastapi
import pydantic
import uvicorn
from android_world import registry as aw_registry_module
from android_world import suite_utils
from android_world.env import env_launcher, interface, json_action


def _env_int(key: str, default: int) -> int:
    val = os.environ.get(key, "")
    return int(val) if val.strip().isdigit() else default


def _env_bool(key: str, default: bool) -> bool:
    val = os.environ.get(key, "").strip().lower()
    if val in ("1", "true", "yes"):
        return True
    if val in ("0", "false", "no"):
        return False
    return default


def _adb_path() -> str:
    explicit = os.environ.get("AW_ADB_PATH", "").strip()
    if explicit:
        return explicit
    # Try to find adb in the cua SDK location
    from pathlib import Path

    candidates = [
        Path.home() / ".cua" / "android-sdk" / "platform-tools" / "adb",
        Path("/opt/android/platform-tools/adb"),
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    import shutil

    found = shutil.which("adb")
    return found or "adb"


class StateResponse(pydantic.BaseModel):
    pixels: list[int]
    ui_elements: list[Any]


@contextlib.asynccontextmanager
async def lifespan(fast_api_app: fastapi.FastAPI):
    console_port = _env_int("AW_CONSOLE_PORT", 5554)
    grpc_port = _env_int("AW_GRPC_PORT", console_port + 3000)
    adb_path = _adb_path()
    emulator_setup = _env_bool("AW_EMULATOR_SETUP", False)
    freeze_datetime = _env_bool("AW_FREEZE_DATETIME", True)

    fast_api_app.state.app_android_env = env_launcher.load_and_setup_env(
        console_port=console_port,
        grpc_port=grpc_port,
        emulator_setup=emulator_setup,
        freeze_datetime=freeze_datetime,
        adb_path=adb_path,
    )
    task_registry = aw_registry_module.TaskRegistry()
    aw_registry = task_registry.get_registry(task_registry.ANDROID_WORLD_FAMILY)
    initial_suite = suite_utils.create_suite(
        task_registry=aw_registry,
        n_task_combinations=2,
        seed=42,
    )
    fast_api_app.state.suite = initial_suite
    fast_api_app.state.task_registry = task_registry
    yield
    if fast_api_app.state.app_android_env is not None:
        fast_api_app.state.app_android_env.close()


app = fastapi.FastAPI(lifespan=lifespan)
suite_router = fastapi.APIRouter(prefix="/suite", tags=["suite"])
task_router = fastapi.APIRouter(prefix="/task", tags=["task"])


def get_app_android_env(request: fastapi.Request) -> interface.AsyncEnv:
    return request.app.state.app_android_env


def get_app_suite(request: fastapi.Request) -> suite_utils.Suite:
    return request.app.state.suite


AndroidEnv = typing.Annotated[interface.AsyncEnv, fastapi.Depends(get_app_android_env)]
AndroidSuite = typing.Annotated[suite_utils.Suite, fastapi.Depends(get_app_suite)]


@app.post("/reset")
async def reset(go_home: bool, app_android_env: AndroidEnv):
    app_android_env.reset(go_home=go_home)
    return {"status": "success", "message": f"Environment reset with go_home={go_home}."}


@app.get("/screenshot")
async def get_screenshot(wait_to_stabilize: bool, app_android_env: AndroidEnv):
    state = app_android_env.get_state(wait_to_stabilize=wait_to_stabilize)
    return {"pixels": state.pixels.tolist()}


@app.post("/execute_action")
async def execute_action(action_dict: dict[str, typing.Any], app_android_env: AndroidEnv):
    action = json_action.JSONAction(**action_dict)
    app_android_env.execute_action(action)
    return {"status": "success", "message": f"Action {action} executed."}


@suite_router.get("/task_list")
async def suite_task_list(max_index: int, app_suite: AndroidSuite):
    if max_index > len(app_suite) or max_index < 0:
        return {"task_list": list(app_suite.keys())}
    return {"task_list": list(app_suite.keys())[:max_index]}


@suite_router.get("/task_length")
async def suite_task_length(task_type: str, app_suite: AndroidSuite):
    return {"length": len(app_suite[task_type])}


@suite_router.get("/reinitialize")
def reinitialize_suite(
    request: fastapi.Request,
    n_task_combinations: int = 2,
    seed: int = 42,
    task_family: str = "android_world",
):
    task_registry = request.app.state.task_registry
    try:
        current_aw_registry = task_registry.get_registry(task_family)
    except ValueError as exc:
        raise fastapi.HTTPException(
            status_code=400, detail=f"Invalid task family: {task_family}"
        ) from exc
    new_suite = suite_utils.create_suite(
        task_registry=current_aw_registry,
        n_task_combinations=n_task_combinations,
        seed=seed,
    )
    request.app.state.suite = new_suite
    return {
        "status": "success",
        "message": f"Suite re-initialized (family={task_family}, seed={seed}).",
    }


@task_router.post("/initialize")
async def initialize_task(
    task_type: str, task_idx: int, app_android_env: AndroidEnv, app_suite: AndroidSuite
):
    app_suite[task_type][task_idx].initialize_task(app_android_env)
    return {"status": "success", "message": f"Task {task_type}[{task_idx}] initialized."}


@task_router.post("/tear_down")
async def tear_down_task(
    task_type: str, task_idx: int, app_android_env: AndroidEnv, app_suite: AndroidSuite
):
    app_suite[task_type][task_idx].tear_down(app_android_env)
    return {"status": "success", "message": f"Task {task_type}[{task_idx}] torn down."}


@task_router.get("/score")
async def get_task_score(
    task_type: str, task_idx: int, app_android_env: AndroidEnv, app_suite: AndroidSuite
):
    return {"score": app_suite[task_type][task_idx].is_successful(app_android_env)}


@task_router.get("/goal")
async def get_task_goal(task_type: str, task_idx: int, app_suite: AndroidSuite):
    return {"goal": app_suite[task_type][task_idx].goal}


@task_router.get("/template")
async def get_task_template(task_type: str, task_idx: int, app_suite: AndroidSuite):
    return {"template": app_suite[task_type][task_idx].template}


@app.post("/close")
async def close(app_android_env: AndroidEnv):
    app_android_env.close()
    return {"status": "success"}


@app.get("/health")
async def health(app_android_env: AndroidEnv):
    if isinstance(app_android_env, interface.AsyncEnv):
        return {"status": "success"}
    raise fastapi.HTTPException(status_code=500, detail="Environment not initialized")


app.include_router(suite_router)
app.include_router(task_router)


if __name__ == "__main__":
    host = os.environ.get("AW_HOST", "0.0.0.0")
    port = _env_int("AW_PORT", 5000)
    uvicorn.run(app, host=host, port=port)
