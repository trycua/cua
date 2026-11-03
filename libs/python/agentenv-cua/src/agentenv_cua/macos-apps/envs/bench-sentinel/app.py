"""The bench's sentinel as a computer app on macOS: BenchSentinel, the native AppKit window that plays the computer's
user and logs all that disturbs that user while an agent works (``~/.bench-sentinel/sentinel.jsonl``, which the bench's
own ``summarize_sentinel.py`` scores). It is the macOS app itself: its install brings its Swift source at the pinned
commit of Bench v2 in trycua/cua (``files.json``) and builds ``BenchSentinel.app`` from it, ad hoc signed.

The condition is a setting: ``foreground``, the "Bench Sentinel" witness at the screen's bottom right, which doesn't
type; ``background``, the user's "User activity" window at the bench's frame, whose field gets one character a second
while it is the active window and key. A task arms it around the agent with the ``sentinel-macos`` artifact, as a ``run_code`` step."""

import json
from pathlib import Path
from typing import Literal

from agentenv_protocol.computer import (
    ComputerAppEnvironment,
    Download,
    Launch,
    Ready,
    RunScript,
    computer_app,
    install,
    install_step,
)
from pydantic import BaseModel, ConfigDict, Field

HERE = Path(__file__).resolve().parent
SUPPORTS = [{"os": {"type": "macos"}, "arch": ["arm64"]}]
LOG_FOLDER = ".bench-sentinel"
FILES = json.loads((HERE / "files.json").read_text(encoding="utf-8"))["files"]
#: The user's window in the background condition, its outer frame's x, y, width and height, as the bench places it.
USER_FRAME = (420, 340, 900, 520)


class MacOS(BaseModel):
    """How a macOS computer installs the app: ``downloads`` fetched by URL and checked by sha256, and ``run`` scripts."""

    model_config = ConfigDict(extra="forbid")

    files: str = "."
    downloads: list[Download] = Field(default_factory=list)
    run: list[RunScript] = Field(default_factory=list)


@computer_app(supports=SUPPORTS)
@install(
    "macos",
    MacOS(
        downloads=[Download(url=f["url"], sha256=f["sha256"], to=f["path"]) for f in FILES],
        run=[RunScript(script="install.sh", scope="session")],
    ),
    launch=Launch(
        at="boot",
        command=["venv/bin/python", "sentinel.py"],
        # Its window's title is its condition's, so it is ready when its log says it started and is sampling.
        ready=Ready(command=["venv/bin/python", "sentinel.py", "--ready"], timeout_s=120),
    ),
    paths=[LOG_FOLDER],
)
class BenchSentinel(ComputerAppEnvironment):
    class Settings(BaseModel):
        #: The bench's two conditions: ``background``, the user's "User activity" window, which types; ``foreground``,
        #: the "Bench Sentinel" witness.
        condition: Literal["foreground", "background"] = "foreground"

    @install_step(scope="session", os="macos")
    def log_folder(self) -> None:
        (Path.home() / LOG_FOLDER).mkdir(exist_ok=True)
