"""trycua's BenchLab as a computer app on macOS: the native AppKit window of the bench's precision and interruption
tasks, MB-09 to MB-11 and IR-01 to IR-04, in the mode and with the seed its settings give. It is the macOS app itself,
not a port: its install brings its three Swift sources at the pinned commit of Bench v2 in trycua/cua (``files.json``,
by URL and sha256, under ``swift/``) and builds ``BenchLab.app`` from them, ad hoc signed, as the bench's own
``swift/build.sh`` does (``build_app.sh``). Its data is ``~/.benchlab``, the state and the events BenchLab writes, which
the bench's evaluator for the task grades. BenchLab places its own window at (60, 80)."""

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
DATA = ".benchlab"
FILES = json.loads((HERE / "files.json").read_text(encoding="utf-8"))["files"]


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
        command=["venv/bin/python", "lab.py"],
        window={"title": "BenchLab"},
        ready=Ready(window=True, timeout_s=120),
    ),
    paths=[DATA],
)
class BenchLab(ComputerAppEnvironment):
    class Settings(BaseModel):
        #: The task's BenchLab mode: MB-09 listdrag, MB-10 hover, MB-11 tooltip, IR-01 irmodal, IR-02 irbanner, IR-03
        #: irunsaved, IR-04 irconsent. The preflight window, hover with seed 1, when a task gives none.
        mode: Literal[
            "listdrag", "hover", "tooltip", "irmodal", "irbanner", "irunsaved", "irconsent"
        ] = "hover"
        #: The trial's seed, the bench's ``probe_seed(task, run_index)``, from which BenchLab draws the task's values.
        seed: int = 1

    @install_step(scope="session", os="macos")
    def data_folder(self) -> None:
        (Path.home() / DATA).mkdir(exist_ok=True)
