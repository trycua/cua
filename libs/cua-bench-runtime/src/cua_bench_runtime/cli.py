"""Command-line interface for the Cua Bench Runtime."""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import NoReturn

from cua_bench_runtime import exit_codes
from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.engine import run_trial
from cua_bench_runtime.errors import CbError, HardAbort, UsageFailure
from cua_bench_runtime.explain import inspect_trial, narrative
from cua_bench_runtime.export_trial import export_trial
from cua_bench_runtime.report import (
    VIEWS,
    build_report,
    build_report_preregistration,
    normalize_trials,
    validate_report_preregistration,
    write_normalized_jsonl,
)
from cua_bench_runtime.receipt_signing import (
    sign_report_preregistration,
    verify_report_preregistration_signature,
)
from cua_bench_runtime.schemas import schema_suite, validate_manifest

KINDS = (
    "task",
    "dataset",
    "driver",
    "profile",
    "system",
    "execution-policy",
    "trial",
    "release",
)


class Parser(argparse.ArgumentParser):
    def error(self, message: str) -> NoReturn:
        raise UsageFailure(message)


def parser() -> Parser:
    root = Parser(prog="cdb")
    commands = root.add_subparsers(dest="command", required=True)

    validate = commands.add_parser("validate", help="validate benchmark manifests")
    validate.add_argument("paths", nargs="*", type=Path)
    validate.add_argument("--kind", choices=KINDS)
    validate.add_argument("--json", action="store_true", dest="as_json")

    run = commands.add_parser("run", help="execute one trial")
    run.add_argument("--task", type=Path, required=True)
    run.add_argument("--agent", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--trial-id")
    run.add_argument("--seed", type=int, default=0)
    run.add_argument("--timeout", type=float)
    run.add_argument("--system", type=Path)
    run.add_argument("--execution-policy", type=Path)
    run.add_argument("--participation-signing-key", type=Path)
    run.add_argument("--participation-verifier-key", type=Path)
    run.add_argument("--lume-config", type=Path)
    run.add_argument("--guest-launch", type=Path)
    run.add_argument("--credential-file", type=Path)
    run.add_argument(
        "--evaluator-node",
        type=Path,
        help="absolute Node executable supplied to evaluators that require it",
    )
    run.add_argument(
        "--evaluator-node-sha256",
        help="expected lowercase SHA-256 of --evaluator-node",
    )
    run.add_argument(
        "--evaluator-node-version",
        help="expected version reported by --evaluator-node (for example v22.23.2)",
    )
    run.add_argument(
        "--agent-brief",
        type=Path,
        help="override production stdin for an explicit apparatus check",
    )
    run.add_argument(
        "--apparatus-check",
        action="store_true",
        help="mark a deterministic apparatus probe that is excluded from reports",
    )
    run.add_argument(
        "--debug",
        action="store_true",
        dest="debug_mode",
        help="retain bounded content-free progress for a non-exportable protected run",
    )
    run.add_argument(
        "--env",
        default="local",
        choices=(
            "local",
            "local-fail-cleanup",
            "local-cua-smoke",
            "task-local-smoke",
            "task-local-cua-smoke",
            "lume-macos",
            "lume-macos-certifying",
        ),
    )
    run.add_argument("--json", action="store_true", dest="as_json")

    explain = commands.add_parser("explain", help="verify and explain a trial directory")
    explain.add_argument("trial_dir", type=Path)
    explain.add_argument("--json", action="store_true", dest="as_json")
    explain.add_argument("--verify-only", action="store_true")

    export = commands.add_parser(
        "export-trial", help="export a verified runtime trial as a v0.3 manifest"
    )
    export.add_argument("--trial-dir", type=Path, required=True)
    export.add_argument("--template", type=Path, required=True)
    export.add_argument("--out", type=Path, required=True)

    preregister = commands.add_parser(
        "preregister-report",
        help="validate and freeze a comparison plan before executing trials",
    )
    preregister.add_argument("--view", choices=tuple(VIEWS), required=True)
    preregister.add_argument("--trial-template", type=Path, action="append", required=True)
    preregister.add_argument("--system", type=Path, action="append", required=True)
    preregister.add_argument("--execution-policy", type=Path, action="append", required=True)
    preregister.add_argument("--signing-key", type=Path, required=True)
    preregister.add_argument("--verifier-key", type=Path, required=True)
    preregister.add_argument("--bootstrap-samples", type=int, default=2000)
    preregister.add_argument("--seed", type=int, default=0)
    preregister.add_argument("--pass-k", type=int, default=5)
    preregister.add_argument("--max-infrastructure-failure-rate", type=float, default=0.05)
    preregister.add_argument("--out", type=Path, required=True)

    report = commands.add_parser(
        "report", help="build a deterministic comparison report from v0.3 trials"
    )
    report.add_argument("--view", choices=tuple(VIEWS), required=True)
    report.add_argument("--trial", type=Path, action="append", required=True)
    report.add_argument("--system", type=Path, action="append", required=True)
    report.add_argument("--execution-policy", type=Path, action="append", required=True)
    report.add_argument("--preregistration", type=Path, required=True)
    report.add_argument("--verifier-key", type=Path, required=True)
    report.add_argument("--out", type=Path, required=True)
    report.add_argument("--normalized-out", type=Path)
    report.add_argument("--bootstrap-samples", type=int, default=2000)
    report.add_argument("--seed", type=int, default=0)
    report.add_argument("--pass-k", type=int, default=5)
    report.add_argument("--max-infrastructure-failure-rate", type=float, default=0.05)
    return root


def _validate(arguments: argparse.Namespace) -> int:
    if not arguments.paths:
        if arguments.as_json:
            with contextlib.redirect_stdout(io.StringIO()):
                return_code = schema_suite()
        else:
            return_code = schema_suite()
        if return_code != 0:
            raise UsageFailure(f"schema suite returned {return_code}")
        report = {"valid": True, "scope": "repository-schema-suite"}
    else:
        items = []
        for path in arguments.paths:
            document = validate_manifest(path, arguments.kind)
            items.append({"path": str(path), "id": document.get("id")})
        report = {"valid": True, "manifests": items}
    if arguments.as_json:
        print(json.dumps(report, sort_keys=True))
    elif arguments.paths:
        print(f"Validated {len(arguments.paths)} manifest(s).")
    return exit_codes.OK


def _run(arguments: argparse.Namespace) -> int:
    code, trial_dir, result = run_trial(
        task_path=arguments.task,
        agent_command=arguments.agent,
        out=arguments.out,
        trial_id=arguments.trial_id,
        seed=arguments.seed,
        timeout_seconds=arguments.timeout,
        environment_name=arguments.env,
        system_path=arguments.system,
        execution_policy_path=arguments.execution_policy,
        participation_signing_key=arguments.participation_signing_key,
        participation_verifier_key=arguments.participation_verifier_key,
        apparatus_check=arguments.apparatus_check,
        lume_config_path=arguments.lume_config,
        guest_launch_path=arguments.guest_launch,
        credential_file_path=arguments.credential_file,
        agent_brief_path=arguments.agent_brief,
        evaluator_node_path=arguments.evaluator_node,
        evaluator_node_sha256=arguments.evaluator_node_sha256,
        evaluator_node_version=arguments.evaluator_node_version,
        debug_mode=arguments.debug_mode,
    )
    if arguments.as_json:
        print(json.dumps({"trial_dir": str(trial_dir), "result": result}, sort_keys=True))
    else:
        print(f"Trial {result['trial_id']}: {result['status']} ({trial_dir})")
    return code


def _explain(arguments: argparse.Namespace) -> int:
    report = inspect_trial(arguments.trial_dir)
    if arguments.as_json:
        print(json.dumps(report, sort_keys=True))
    elif not arguments.verify_only:
        print(narrative(report))
    return exit_codes.OK


def _report(arguments: argparse.Namespace) -> int:
    rows = normalize_trials(
        arguments.trial,
        arguments.system,
        arguments.execution_policy,
        certification_verifier_key=arguments.verifier_key,
    )
    try:
        signed_preregistration = json.loads(arguments.preregistration.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise UsageFailure("report preregistration is not readable JSON") from error
    preregistration = verify_report_preregistration_signature(
        signed_preregistration,
        trusted_public_key=arguments.verifier_key,
    )
    validate_report_preregistration(
        preregistration,
        rows,
        arguments.system,
        arguments.execution_policy,
        arguments.view,
        bootstrap_samples=arguments.bootstrap_samples,
        seed=arguments.seed,
        pass_k=arguments.pass_k,
        max_infrastructure_failure_rate=arguments.max_infrastructure_failure_rate,
    )
    report = build_report(
        rows,
        arguments.view,
        bootstrap_samples=arguments.bootstrap_samples,
        seed=arguments.seed,
        pass_k=arguments.pass_k,
        max_infrastructure_failure_rate=arguments.max_infrastructure_failure_rate,
    )
    report["preregistration_digest"] = preregistration["digest"]
    report.pop("digest", None)
    report["digest"] = digest_json(report)
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    arguments.out.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if arguments.normalized_out:
        write_normalized_jsonl(rows, arguments.normalized_out)
    print(json.dumps({"report": str(arguments.out), "digest": report["digest"]}, sort_keys=True))
    return exit_codes.OK


def _preregister_report(arguments: argparse.Namespace) -> int:
    document = build_report_preregistration(
        arguments.trial_template,
        arguments.system,
        arguments.execution_policy,
        arguments.view,
        bootstrap_samples=arguments.bootstrap_samples,
        seed=arguments.seed,
        pass_k=arguments.pass_k,
        max_infrastructure_failure_rate=arguments.max_infrastructure_failure_rate,
    )
    signed = sign_report_preregistration(document, private_key=arguments.signing_key)
    verified = verify_report_preregistration_signature(
        signed,
        trusted_public_key=arguments.verifier_key,
    )
    if verified != document:
        raise UsageFailure("signed report preregistration changed during verification")
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    try:
        with arguments.out.open("x", encoding="utf-8") as output:
            json.dump(signed, output, indent=2, sort_keys=True)
            output.write("\n")
    except FileExistsError as error:
        raise UsageFailure(f"report preregistration already exists: {arguments.out}") from error
    print(
        json.dumps(
            {"preregistration": str(arguments.out), "digest": document["digest"]},
            sort_keys=True,
        )
    )
    return exit_codes.OK


def main(argv: Sequence[str] | None = None) -> int:
    try:
        arguments = parser().parse_args(argv)
        if arguments.command == "validate":
            return _validate(arguments)
        if arguments.command == "run":
            return _run(arguments)
        if arguments.command == "explain":
            return _explain(arguments)
        if arguments.command == "export-trial":
            export_trial(arguments.trial_dir, arguments.template, arguments.out)
            print(json.dumps({"trial": str(arguments.out)}, sort_keys=True))
            return exit_codes.OK
        if arguments.command == "preregister-report":
            return _preregister_report(arguments)
        if arguments.command == "report":
            return _report(arguments)
        raise UsageFailure(f"unknown command: {arguments.command}")
    except HardAbort as error:
        print(f"cdb: {error}", file=sys.stderr)
        return error.exit_code
    except CbError as error:
        print(f"cdb: {error}", file=sys.stderr)
        return error.exit_code
    except KeyboardInterrupt:
        return exit_codes.HARD_ABORT
