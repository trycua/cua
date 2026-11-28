"""Load and validate prompt test specs (tests/agentic/<area>/<name>.yaml).

The validator is the schema: every key a spec may use is listed here, and
anything else is an error, so a typo never silently turns a check off.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[1]  # tests/agentic
OSES = ("windows", "linux", "macos")
HOST_ROLES = ("app", "computer")
HOST_ENVS = ("staging", "production-names")
HOST_HOMES = ("isolated", "default")
CHECK_TYPES = ("relay", "cli", "ui", "no_dialogs", "file")
TEMPLATE = re.compile(r"\{\{\s*([a-z0-9_.]+)\s*\}\}")
TEMPLATE_VARS = re.compile(r"^(run|host\.[1-9]\.(name|home|machine_id|initial_machine_id))$")


class SpecError(ValueError):
    pass


@dataclass
class Host:
    id: int
    role: str = "computer"
    env: str = "staging"
    home: str = "isolated"
    signed_in: bool = True
    joined: bool = False
    terminal: str | None = None
    # How a joined host runs: `real` uses this OS's service manager (Task
    # Scheduler, systemd, the app helper), `process` a plain process that
    # stands in for another computer and can be "turned off".
    service: str = ""
    experiments: list[str] = field(default_factory=list)


@dataclass
class Spec:
    path: Path
    name: str
    area: str
    os: list[str]
    prompt: str
    hosts: list[Host]
    build: str
    account: str
    steps: int
    minutes: float
    expect: list[dict[str, Any]]
    issues: list[str] = field(default_factory=list)
    title: str = ""

    @property
    def key(self) -> str:
        return f"{self.area}/{self.name}"

    def host(self, n: int) -> Host:
        for h in self.hosts:
            if h.id == n:
                return h
        raise SpecError(f"{self.key}: no host {n}")


def _keys(where: str, d: Any, required: set[str], optional: set[str]) -> None:
    if not isinstance(d, dict):
        raise SpecError(f"{where}: expected a mapping, got {type(d).__name__}")
    missing = required - d.keys()
    if missing:
        raise SpecError(f"{where}: missing {', '.join(sorted(missing))}")
    unknown = d.keys() - required - optional
    if unknown:
        raise SpecError(f"{where}: unknown key(s) {', '.join(sorted(unknown))}")


def _str_list(where: str, v: Any) -> list[str]:
    if isinstance(v, str):
        return [v]
    if isinstance(v, list) and all(isinstance(x, str) and x for x in v):
        return list(v)
    raise SpecError(f"{where}: expected a string or a list of strings")


def _check_templates(where: str, value: Any, host_ids: set[int]) -> None:
    if isinstance(value, str):
        for var in TEMPLATE.findall(value):
            if not TEMPLATE_VARS.match(var):
                raise SpecError(f"{where}: unknown template variable {{{{{var}}}}}")
            if var.startswith("host.") and int(var.split(".")[1]) not in host_ids:
                raise SpecError(f"{where}: {{{{{var}}}}} names a host the setup does not have")
    elif isinstance(value, list):
        for i, v in enumerate(value):
            _check_templates(f"{where}[{i}]", v, host_ids)
    elif isinstance(value, dict):
        for k, v in value.items():
            _check_templates(f"{where}.{k}", v, host_ids)


def _host_ref(where: str, check: dict, host_ids: set[int]) -> None:
    n = check.get("host", 1)
    if not isinstance(n, int) or n not in host_ids:
        raise SpecError(f"{where}.host: {n!r} is not one of the setup's hosts {sorted(host_ids)}")


def _validate_check(where: str, item: Any, host_ids: set[int], oses: list[str]) -> dict:
    if not isinstance(item, dict) or len(item) != 1:
        raise SpecError(
            f"{where}: each check is a mapping with exactly one of {', '.join(CHECK_TYPES)}"
        )
    ((kind, body),) = item.items()
    if kind not in CHECK_TYPES:
        raise SpecError(f"{where}: unknown check type {kind!r} (one of {', '.join(CHECK_TYPES)})")
    w = f"{where}.{kind}"
    common = {"name", "os"}
    if kind == "relay":
        _keys(w, body, {"get", "jq"}, common | {"within_s"})
        if not str(body["get"]).startswith("/v1/"):
            raise SpecError(f"{w}.get: a relay API path such as /v1/machines")
    elif kind == "cli":
        _keys(w, body, {"run", "jq"}, common | {"host", "within_s"})
        _host_ref(w, body, host_ids)
        run = body["run"]
        argv = run.split() if isinstance(run, str) else run
        if not argv or argv[0] != "cua" or "--json" not in argv:
            raise SpecError(f"{w}.run: a `cua ... --json` command")
    elif kind == "ui":
        _keys(
            w,
            body,
            {"app"},
            common | {"host", "contains", "absent", "row", "within_s", "hold_s", "after_event"},
        )
        _host_ref(w, body, host_ids)
        if not any(k in body for k in ("contains", "absent", "row")):
            raise SpecError(f"{w}: needs contains, absent or row")
        for k in ("contains", "absent"):
            if k in body:
                _str_list(f"{w}.{k}", body[k])
        if "row" in body:
            _keys(f"{w}.row", body["row"], {"has"}, {"and"})
            _str_list(f"{w}.row.has", body["row"]["has"])
            if "and" in body["row"]:
                _str_list(f"{w}.row.and", body["row"]["and"])
        ev = body.get("after_event")
        if ev is not None and not re.fullmatch(r"power_(off|on):[1-9]", str(ev)):
            raise SpecError(f"{w}.after_event: power_off:N or power_on:N")
    elif kind == "no_dialogs":
        _keys(w, body, {"titles"}, common | {"host"})
        _str_list(f"{w}.titles", body["titles"])
        _host_ref(w, body, host_ids)
    elif kind == "file":
        _keys(w, body, {"path"}, common | {"host", "exists", "contains", "absent"})
        _host_ref(w, body, host_ids)
    for k in ("within_s", "hold_s"):
        if k in body and (not isinstance(body[k], (int, float)) or body[k] < 0 or body[k] > 900):
            raise SpecError(f"{w}.{k}: seconds between 0 and 900")
    if "os" in body:
        only = _str_list(f"{w}.os", body["os"])
        bad = [o for o in only if o not in oses]
        if bad:
            raise SpecError(f"{w}.os: {bad} is not in the test's os list")
    _check_templates(w, body, host_ids)
    return {"type": kind, **body}


def load(path: Path) -> Spec:
    path = Path(path)
    try:
        doc = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as e:
        raise SpecError(f"{path}: not valid YAML: {e}") from None
    where = str(path)
    _keys(where, doc, {"name", "os", "setup", "prompt", "budget", "expect"}, {"title", "issues"})
    name = doc["name"]
    if not isinstance(name, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", name):
        raise SpecError(f"{where}.name: lowercase letters, digits and dashes")
    if path.stem != name:
        raise SpecError(f"{where}: the file must be named {name}.yaml")
    area = path.parent.name
    oses = _str_list(f"{where}.os", doc["os"])
    for o in oses:
        if o not in OSES:
            raise SpecError(f"{where}.os: {o!r} is not one of {', '.join(OSES)}")

    setup = doc["setup"]
    _keys(f"{where}.setup", setup, {"build", "account", "hosts"}, set())
    if setup["build"] != "spaces-app":
        raise SpecError(f"{where}.setup.build: only spaces-app is supported")
    if setup["account"] != "canary":
        raise SpecError(f"{where}.setup.account: only the canary test account is allowed")
    raw_hosts = setup["hosts"]
    if isinstance(raw_hosts, int):
        raw_hosts = [
            {"id": i + 1, "role": "app" if i == 0 else "computer"} for i in range(raw_hosts)
        ]
    if not isinstance(raw_hosts, list) or not 1 <= len(raw_hosts) <= 4:
        raise SpecError(f"{where}.setup.hosts: a count (1-4) or a list of 1-4 hosts")
    hosts: list[Host] = []
    for i, h in enumerate(raw_hosts):
        hw = f"{where}.setup.hosts[{i}]"
        _keys(
            hw,
            h,
            {"id"},
            {"role", "env", "home", "signed_in", "joined", "terminal", "service", "experiments"},
        )
        host = Host(
            id=h["id"],
            role=h.get("role", "app" if h["id"] == 1 else "computer"),
            env=h.get("env", "staging"),
            home=h.get("home", "isolated"),
            signed_in=h.get("signed_in", True),
            joined=h.get("joined", False),
            terminal=h.get("terminal"),
            service=h.get("service", ""),
            experiments=(
                _str_list(f"{hw}.experiments", h["experiments"]) if "experiments" in h else []
            ),
        )
        if host.id != i + 1:
            raise SpecError(f"{hw}.id: hosts are numbered 1, 2, ... in order")
        if host.role not in HOST_ROLES:
            raise SpecError(f"{hw}.role: one of {', '.join(HOST_ROLES)}")
        if not host.service:
            host.service = "real" if host.role == "app" else "process"
        if host.service not in ("real", "process"):
            raise SpecError(f"{hw}.service: real or process")
        if host.env not in HOST_ENVS:
            raise SpecError(f"{hw}.env: one of {', '.join(HOST_ENVS)}")
        if host.home not in HOST_HOMES:
            raise SpecError(f"{hw}.home: one of {', '.join(HOST_HOMES)}")
        if host.joined and not host.signed_in:
            raise SpecError(f"{hw}: a joined host must be signed in")
        if host.terminal is not None and (
            not isinstance(host.terminal, str) or not host.terminal.strip()
        ):
            raise SpecError(f"{hw}.terminal: the terminal window's title")
        hosts.append(host)
    if sum(h.role == "app" for h in hosts) != 1:
        raise SpecError(
            f"{where}.setup.hosts: exactly one host has role app (the one the agent drives)"
        )
    host_ids = {h.id for h in hosts}

    prompt = doc["prompt"]
    if not isinstance(prompt, str) or len(prompt.strip()) < 20:
        raise SpecError(f"{where}.prompt: a plain-English goal of at least a sentence")
    _check_templates(f"{where}.prompt", prompt, host_ids)

    budget = doc["budget"]
    _keys(f"{where}.budget", budget, {"steps", "minutes"}, set())
    steps, minutes = budget["steps"], budget["minutes"]
    if not isinstance(steps, int) or not 1 <= steps <= 300:
        raise SpecError(f"{where}.budget.steps: 1-300 tool calls")
    if not isinstance(minutes, (int, float)) or not 1 <= minutes <= 60:
        raise SpecError(f"{where}.budget.minutes: 1-60")

    expect = doc["expect"]
    if not isinstance(expect, list) or not expect:
        raise SpecError(f"{where}.expect: at least one hard check")
    checks = [
        _validate_check(f"{where}.expect[{i}]", c, host_ids, oses) for i, c in enumerate(expect)
    ]

    return Spec(
        path=path,
        name=name,
        area=area,
        os=oses,
        prompt=prompt.strip(),
        hosts=hosts,
        build=setup["build"],
        account=setup["account"],
        steps=steps,
        minutes=float(minutes),
        expect=checks,
        issues=_str_list(f"{where}.issues", doc["issues"]) if "issues" in doc else [],
        title=str(doc.get("title", "")),
    )


def discover(root: Path = ROOT) -> list[Path]:
    return sorted(
        p for p in root.glob("*/*.yaml") if p.parent.name not in ("trajectories", "runner")
    )


def render(value: Any, variables: dict[str, str]) -> Any:
    """Fill {{...}} template variables; an unknown one at run time is an error."""
    if isinstance(value, str):

        def sub(m: re.Match) -> str:
            var = m.group(1)
            if var not in variables:
                raise SpecError(f"template variable {{{{{var}}}}} has no value in this run")
            return variables[var]

        return TEMPLATE.sub(sub, value)
    if isinstance(value, list):
        return [render(v, variables) for v in value]
    if isinstance(value, dict):
        return {k: render(v, variables) for k, v in value.items()}
    return value
