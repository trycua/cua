#!/usr/bin/env python3
"""Validate executable Task 11 binding-contract fixture structure."""

from __future__ import annotations

import argparse
import ast
import io
import re
import sys
import tempfile
import tokenize
from pathlib import Path


class ContractFailure(RuntimeError):
    pass


KOTLIN_BUILD_CHECKS = [
    ("Kotlin test source set", r"sourceSets\.test\s*\{\s*kotlin\.srcDirs\(\"tests\"\)"),
    ("Kotlin example source set", r"sourceSets\.creating\s*\{\s*kotlin\.srcDir\(\"\.\./examples/kotlin\"\)"),
    ("Kotlin example compile main output", r"compileClasspath\s*\+=\s*sourceSets\.main\.get\(\)\.output"),
    ("Kotlin example runtime main output", r"runtimeClasspath\s*\+=\s*sourceSets\.main\.get\(\)\.output"),
    ("Kotlin example implementation inheritance", r"configurations\[example\.implementationConfigurationName\]\.extendsFrom\(\s*configurations\[sourceSets\.main\.get\(\)\.implementationConfigurationName\]"),
    ("Kotlin example runtime inheritance", r"configurations\[example\.runtimeOnlyConfigurationName\]\.extendsFrom\(\s*configurations\[sourceSets\.main\.get\(\)\.runtimeOnlyConfigurationName\]"),
    ("Kotlin example classes dependency", r"dependsOn\(example\.classesTaskName\)"),
    ("Kotlin example runtime classpath", r"classpath\s*=\s*example\.runtimeClasspath"),
]


def strip_python_comments_and_strings(text: str) -> str:
    tokens = []
    for token in tokenize.generate_tokens(io.StringIO(text).readline):
        if token.type in {tokenize.COMMENT, tokenize.STRING}:
            token = tokenize.TokenInfo(token.type, "\n" * token.string.count("\n"), token.start, token.end, token.line)
        tokens.append(token)
    return tokenize.untokenize(tokens)


def strip_c_like_comments(text: str) -> str:
    output = []
    index = 0
    state = "code"
    block_comment_depth = 0
    while index < len(text):
        if state == "code" and text.startswith("//", index):
            state = "line_comment"
            index += 2
        elif state == "code" and text.startswith("/*", index):
            state = "block_comment"
            block_comment_depth = 1
            index += 2
        elif state == "code" and text.startswith('\"\"\"', index):
            state = "triple_string"
            output.append('\"\"\"')
            index += 3
        elif state == "triple_string" and text.startswith('\"\"\"', index):
            state = "code"
            output.append('\"\"\"')
            index += 3
        elif state == "code" and text[index] in {'\"', "'"}:
            state = text[index]
            output.append(text[index])
            index += 1
        elif state in {'\"', "'"}:
            output.append(text[index])
            if text[index] == "\\" and index + 1 < len(text):
                output.append(text[index + 1])
                index += 2
            elif text[index] == state:
                state = "code"
                index += 1
            else:
                index += 1
        elif state == "line_comment":
            if text[index] == "\n":
                output.append("\n")
                state = "code"
            index += 1
        elif state == "block_comment":
            if text.startswith("/*", index):
                block_comment_depth += 1
                index += 2
            elif text.startswith("*/", index):
                block_comment_depth -= 1
                index += 2
                if block_comment_depth == 0:
                    state = "code"
            else:
                if text[index] == "\n":
                    output.append("\n")
                index += 1
        else:
            output.append(text[index])
            index += 1
    return "".join(output)


def strip_ruby_comments(text: str) -> str:
    output = []
    in_block_comment = False
    for line in text.splitlines(keepends=True):
        if re.match(r"^\s*=begin\s*$", line):
            in_block_comment = True
            continue
        if in_block_comment:
            if re.match(r"^\s*=end\s*$", line):
                in_block_comment = False
            continue
        quote = None
        escaped = False
        kept = []
        for character in line:
            if quote is None and character == "#":
                break
            kept.append(character)
            if quote is None and character in {"'", '\"'}:
                quote = character
            elif quote is not None:
                if escaped:
                    escaped = False
                elif character == "\\":
                    escaped = True
                elif character == quote:
                    quote = None
        output.append("".join(kept))
    return "".join(output)


def strip_yaml_comments(text: str) -> str:
    output = []
    quote = None
    for line in text.splitlines(keepends=True):
        kept = []
        index = 0
        while index < len(line):
            character = line[index]
            if quote is None and character == "#" and (index == 0 or line[index -1].isspace()):
                break
            kept.append(character)
            if quote is None and character in {"'", '"'}:
                quote = character
            elif quote == "'":
                if character == "'" and index + 1 < len(line) and line[index + 1] == "'":
                    kept.append(line[index + 1])
                    index += 1
                elif character == "'":
                    quote = None
            elif character == "\\" and index + 1 < len(line):
                kept.append(line[index + 1])
                index += 1
            elif character == '"':
                quote = None
            index += 1
        output.append("".join(kept))
    return "".join(output)


def executable_source(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".py":
        return strip_python_comments_and_strings(text)
    if path.suffix in {".kt", ".kts", ".swift"}:
        return strip_c_like_comments(text)
    if path.suffix == ".rb":
        return strip_ruby_comments(text)
    if path.suffix in {".yml", ".yaml"}:
        return strip_yaml_comments(text)
    return text


def require(source: str, description: str, pattern: str, path: Path) -> None:
    if not re.search(pattern, source, flags=re.MULTILINE | re.DOTALL):
        raise ContractFailure(f"binding contract check failed: {description} ({path})")


def require_all(source: str, path: Path, checks: list[tuple[str, str]]) -> None:
    for description, pattern in checks:
        require(source, description, pattern, path)


def forbid(source: str, description: str, pattern: str, path: Path) -> None:
    if re.search(pattern, source, flags=re.MULTILINE | re.DOTALL):
        raise ContractFailure(f"binding contract check failed: {description} ({path})")


def require_kotlin_build(path: Path) -> None:
    require_all(executable_source(path), path, KOTLIN_BUILD_CHECKS)


def yaml_mapping_block(source: str, key: str, indent: int, path: Path) -> str:
    lines = source.splitlines(keepends=True)
    prefix = " " * indent + f"{key}:"
    start = next(
        (
            index
            for index, line in enumerate(lines)
            if line.startswith(prefix) and not line[len(prefix):].strip()
        ),
        None,
    )
    if start is None:
        raise ContractFailure(f"binding contract check failed: YAML block {key} ({path})")

    end = len(lines)
    for index in range(start + 1, len(lines)):
        line = lines[index]
        if line.strip() and len(line) - len(line.lstrip(" ")) <= indent:
            end = index
            break
    return "".join(lines[start:end])


def yaml_job_block(source: str, name: str, path: Path) -> str:
    return yaml_mapping_block(yaml_mapping_block(source, "jobs", 0, path), name, 2, path)


def yaml_step_blocks(job: str, path: Path) -> list[str]:
    steps = yaml_mapping_block(job, "steps", 4, path)
    lines = steps.splitlines(keepends=True)
    starts = [index for index, line in enumerate(lines) if line.startswith("      - ")]
    if not starts:
        raise ContractFailure(f"binding contract check failed: YAML step blocks ({path})")
    return ["".join(lines[start:end]) for start, end in zip(starts, starts[1:] + [len(lines)])]


def yaml_named_step(steps: list[str], name: str, path: Path) -> str:
    pattern = rf"^      - name:\s*{re.escape(name)}\s*$"
    for step in steps:
        if re.search(pattern, step, flags=re.MULTILINE):
            return step
    raise ContractFailure(f"binding contract check failed: workflow step {name} ({path})")


def is_trusted_uniffi_ec2_event(
    event_name: str,
    is_fork: bool,
    head_repository: str,
    repository: str,
    actor: str,
) -> bool:
    return event_name != "pull_request" or (
        not is_fork and head_repository == repository and actor != "dependabot[bot]"
    )


def sdk_bindings_route(
    trusted_event: bool,
    start_result: str,
    run_attempt: str,
    run_id: str,
) -> tuple[bool, str | None]:
    if not trusted_event:
        return True, "ubuntu-24.04"
    if start_result == "success":
        return True, f"cyclops-sdk-diag-{run_attempt}-{run_id}"
    return False, None


def require_sdk_bindings_workflow(path: Path) -> None:
    source = executable_source(path)
    permissions = yaml_mapping_block(source, "permissions", 0, path)
    environment = yaml_mapping_block(source, "env", 0, path)
    start = yaml_job_block(source, "start-sdk-bindings-runner", path)
    sdk = yaml_job_block(source, "sdk-bindings", path)
    stop = yaml_job_block(source, "stop-sdk-bindings-runner", path)
    diagnostics_tests = yaml_job_block(source, "uniffi-diagnostics-tests", path)

    for trigger_path in (
        ".github/scripts/uniffi-diagnostics/**",
        ".github/scripts/tests/test_run_with_otel.py",
    ):
        if source.count(f'"{trigger_path}"') != 2:
            raise ContractFailure(
                f"binding contract check failed: diagnostic path trigger {trigger_path} ({path})"
            )

    require_all(permissions, path, [
        ("workflow contents permission", r"^  contents:\s*read\s*$"),
    ])
    forbid(permissions, "workflow-level EC2 OIDC permission", r"^  id-token:\s*write\s*$", path)
    require_all(environment, path, [
        ("EC2 AWS region", r"^  AWS_REGION:\s*us-west-2\s*$"),
        ("EC2 AWS account", r"^  AWS_ACCOUNT_ID:\s*['\"]?296062593712['\"]?\s*$"),
        ("EC2 runner role", r"^  EC2_RUNNER_IAM_ROLE:\s*github-actions-nixos-ami\s*$"),
        ("EC2 diagnostic instance type", r"^  EC2_RUNNER_INSTANCE_TYPE:\s*m7i\.2xlarge\s*$"),
        ("EC2 runner subnet", r"^  EC2_RUNNER_SUBNET_ID:\s*subnet-0ca86dea50cd84d24\s*$"),
        ("EC2 runner security group", r"^  EC2_RUNNER_SECURITY_GROUP_ID:\s*sg-078f4e7b96883e60e\s*$"),
    ])

    diagnostics_test_steps = yaml_step_blocks(diagnostics_tests, path)
    diagnostics_test_checkout = yaml_named_step(diagnostics_test_steps, "Checkout code", path)
    diagnostics_test_python = yaml_named_step(diagnostics_test_steps, "Set up Python", path)
    diagnostics_test_run = yaml_named_step(diagnostics_test_steps, "Run UniFFI diagnostic wrapper tests", path)
    require_all(diagnostics_tests, path, [
        ("diagnostic test runner host", r"^    runs-on:\s*ubuntu-latest\s*$"),
    ])
    require_all(diagnostics_test_checkout, path, [
        ("diagnostic test checkout", r"uses:\s*actions/checkout@[^\n]+"),
    ])
    require_all(diagnostics_test_python, path, [
        ("diagnostic test Python setup", r"uses:\s*actions/setup-python@[^\n]+"),
        ("diagnostic test Python version", r"python-version:\s*['\"]?3\.11['\"]?"),
    ])
    require_all(diagnostics_test_run, path, [
        ("diagnostic test requirements", r"python3\s+-m\s+pip\s+install\s+--disable-pip-version-check\s+-r\s+\.github/scripts/uniffi-diagnostics/requirements\.txt"),
        ("diagnostic wrapper test command", r"python3\s+\.github/scripts/tests/test_run_with_otel\.py\s+-v"),
        ("diagnostic workflow contract command", r"cyclops-cs/scripts/check-sdk-binding-contract-sources\.sh"),
        ("diagnostic workflow contract self-test", r"cyclops-cs/scripts/check-sdk-binding-contract-sources\.sh\s+--self-test"),
    ])

    start_permissions = yaml_mapping_block(start, "permissions", 4, path)
    start_steps = yaml_step_blocks(start, path)
    start_credentials = yaml_named_step(start_steps, "Configure AWS credentials", path)
    ami_lookup = yaml_named_step(start_steps, "Get latest Ubuntu 24.04 AMI", path)
    start_runner = yaml_named_step(start_steps, "Start EC2 runner", path)
    resolve = yaml_named_step(start_steps, "Resolve EC2 runner instance", path)
    require_all(start, path, [
        ("EC2 start waits for diagnostic tests", r"^    needs:\s*uniffi-diagnostics-tests\s*$"),
        ("EC2 start runner host", r"^    runs-on:\s*ubuntu-latest\s*$"),
        ('trusted EC2 start guard', "^    if:\\s*\\$\\{\\{\\s*github\\.event_name\\s*!=\\s*'pull_request'\\s*\\|\\|\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*==\\s*github\\.repository\\s*&&\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*==\\s*false\\s*&&\\s*github\\.actor\\s*!=\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\}\\}\\s*$"),
        ("EC2 start instance output", r"^      ec2-instance-id:\s*\$\{\{\s*steps\.resolve-ec2\.outputs\.ec2-instance-id\s*\}\}\s*$"),
    ])
    forbid(start, "EC2 action output reliance", r"steps\.start-ec2\.outputs\.ec2-instance-id", path)
    require_all(start_permissions, path, [
        ("EC2 start OIDC permission", r"^      id-token:\s*write\s*$"),
    ])
    require_all(start_credentials, path, [
        ("EC2 start AWS credentials action", r"uses:\s*aws-actions/configure-aws-credentials@ff717079ee2060e4bcee96c4779b553acc87447c"),
        ("EC2 start AWS role", r"role-to-assume:\s*arn:aws:iam::\$\{\{\s*env\.AWS_ACCOUNT_ID\s*\}\}:role/\$\{\{\s*env\.EC2_RUNNER_IAM_ROLE\s*\}\}"),
        ("EC2 start AWS region", r"aws-region:\s*\$\{\{\s*env\.AWS_REGION\s*\}\}"),
    ])
    require_all(ami_lookup, path, [
        ("EC2 AMI lookup id", r"id:\s*ami\s*$"),
        ("EC2 Ubuntu AMI lookup", r"aws ec2 describe-images"),
        ("EC2 Ubuntu AMI owner", r"--owners\s+099720109477"),
        ("EC2 Ubuntu Noble AMI filter", r"--filters\s+\"Name=name,Values=ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24\.04-amd64-server-\*\""),
        ("EC2 latest AMI query", r"--query\s+'sort_by\(Images,\s*&CreationDate\)\[-1\]\.ImageId'"),
        ("EC2 AMI nonempty validation", r"test\s+-n\s+\"\$AMI_ID\""),
        ("EC2 AMI None validation", r"test\s+\"\$AMI_ID\"\s*!=\s*\"None\""),
        ("EC2 Ubuntu AMI output", r"echo\s+\"ami_id=\$AMI_ID\"\s*>>\s*\"\$GITHUB_OUTPUT\""),
    ])
    require_all(start_runner, path, [
        ("EC2 start action id", r"id:\s*start-ec2\s*$"),
        ("EC2 start action SHA", r"uses:\s*unblocked/ec2-action-builder@504a3cf746c6090065a66896a3addc2bb101b4ba"),
        ("EC2 runner PAT secret", r"github_token:\s*\$\{\{\s*secrets\.QEMU_BUILD_GH_PERSONAL_ACCESS_TOKEN\s*\}\}"),
        ("EC2 runner AWS access key", r"aws_access_key_id:\s*\$\{\{\s*env\.AWS_ACCESS_KEY_ID\s*\}\}"),
        ("EC2 runner AWS secret key", r"aws_secret_access_key:\s*\$\{\{\s*env\.AWS_SECRET_ACCESS_KEY\s*\}\}"),
        ("EC2 runner AWS session token", r"aws_session_token:\s*\$\{\{\s*env\.AWS_SESSION_TOKEN\s*\}\}"),
        ("EC2 runner AWS region", r"aws_region:\s*\$\{\{\s*env\.AWS_REGION\s*\}\}"),
        ("EC2 runner subnet wiring", r"ec2_subnet_id:\s*\$\{\{\s*env\.EC2_RUNNER_SUBNET_ID\s*\}\}"),
        ("EC2 runner security group wiring", r"ec2_security_group_id:\s*\$\{\{\s*env\.EC2_RUNNER_SECURITY_GROUP_ID\s*\}\}"),
        ("EC2 runner instance type wiring", r"ec2_instance_type:\s*\$\{\{\s*env\.EC2_RUNNER_INSTANCE_TYPE\s*\}\}"),
        ("EC2 runner AMI wiring", r"ec2_ami_id:\s*\$\{\{\s*steps\.ami\.outputs\.ami_id\s*\}\}"),
        ("EC2 runner root disk", r"ec2_root_disk_size_gb:\s*['\"]?60['\"]?"),
        ("EC2 on-demand strategy", r"ec2_spot_instance_strategy:\s*None"),
        ("EC2 runner TTL", r"ec2_instance_ttl:\s*60"),
        ("EC2 runner version", r"github_action_runner_version:\s*['\"]?2\.333\.1['\"]?"),
        ("EC2 attempt-unique label", r"github_action_runner_label_prefix:\s*cyclops-sdk-diag-\$\{\{\s*github\.run_attempt\s*\}\}-"),
        ("EC2 instance tags", r"ec2_instance_tags:\s*>-"),
        ("EC2 run-attempt tag", r"\"Key\":\s*\"github_run_attempt\"\s*,\s*\"Value\":\s*\"\$\{\{\s*github\.run_attempt\s*\}\}"),
        ("EC2 repository ID tag", r"\"Key\":\s*\"github_repository_id\"\s*,\s*\"Value\":\s*\"\$\{\{\s*github\.repository_id\s*\}\}"),
        ("EC2 diagnostic marker tag", r"\"Key\":\s*\"uniffi_diagnostic\"\s*,\s*\"Value\":\s*\"true\""),
    ])
    require_all(resolve, path, [
        ("EC2 resolve step id", r"id:\s*resolve-ec2\s*$"),
        ("unconditional EC2 resolve", r"^        if:\s*always\(\)\s*$"),
        ("EC2 resolve query", r"aws\s+ec2\s+describe-instances"),
        ("EC2 resolve run-id tag", r"Name=tag:github_job_id,Values=\$\{\{\s*github\.run_id\s*\}\}"),
        ("EC2 resolve repository tag", r"Name=tag:github_repo,Values=\$\{\{\s*github\.event\.repository\.name\s*\}\}"),
        ("EC2 resolve repository ID tag", r"Name=tag:github_repository_id,Values=\$\{\{\s*github\.repository_id\s*\}\}"),
        ("EC2 resolve run-attempt tag", r"Name=tag:github_run_attempt,Values=\$\{\{\s*github\.run_attempt\s*\}\}"),
        ("EC2 resolve diagnostic tag", r"Name=tag:uniffi_diagnostic,Values=true"),
        ("EC2 resolved instance output", r"echo\s+\"ec2-instance-id=\$INSTANCE_ID\"\s*>>\s*\"\$GITHUB_OUTPUT\""),
    ])

    sdk_steps = yaml_step_blocks(sdk, path)
    prerequisites = yaml_named_step(sdk_steps, "Install EC2 runner prerequisites", path)
    gradle_setup = yaml_named_step(sdk_steps, "Set up Gradle", path)
    if sdk_steps.index(prerequisites) >= sdk_steps.index(gradle_setup):
        raise ContractFailure(f"binding contract check failed: EC2 runner prerequisites ordering ({path})")
    require_all(prerequisites, path, [
        ('self-hosted EC2 runner prerequisite route', "^        if:\s*\$\{\{\s*runner\.environment\s*==\s*'self-hosted'\s*\}\}\s*$"),
        ("EC2 runner apt metadata refresh", r"sudo\s+apt-get\s+update"),
        ("EC2 runner unzip prerequisite", r"sudo\s+apt-get\s+install\s+--yes\s+unzip"),
    ])

    checkout = yaml_named_step(sdk_steps, "Checkout code", path)
    node_setup = yaml_named_step(sdk_steps, "Set up Node.js", path)
    binding_harness = yaml_named_step(sdk_steps, "Check generated bindings and generator regression harness", path)
    if not (sdk_steps.index(checkout) < sdk_steps.index(node_setup) < sdk_steps.index(binding_harness)):
        raise ContractFailure(f"binding contract check failed: SDK Node setup ordering ({path})")
    forbid(node_setup, "conditional SDK Node setup", r"^        if:", path)
    require_all(node_setup, path, [
        ("SDK Node setup action", r"uses:\s*actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020"),
        ("SDK Node version file", r"node-version-file:\s*['\"]?cyclops-cs/\.nvmrc['\"]?"),
    ])

    go_index = next(
        (index for index, step in enumerate(sdk_steps) if "uses: actions/setup-go@" in step),
        None,
    )
    if go_index is None:
        raise ContractFailure(f"binding contract check failed: SDK bindings Go setup ({path})")
    go_setup = sdk_steps[go_index]
    require_all(go_setup, path, [
        ("SDK bindings Go setup action", r"uses:\s*actions/setup-go@[^\n]+"),
        ("SDK bindings Go version file", r"go-version-file:\s*cyclops-cs/backend/go\.mod"),
    ])

    python_index = next(
        (index for index, step in enumerate(sdk_steps) if re.search(r"^      - name:\s*Set up Python\s*$", step, flags=re.MULTILINE)),
        None,
    )
    if python_index is None or python_index + 1 == len(sdk_steps):
        raise ContractFailure(f"binding contract check failed: SDK diagnostics placement ({path})")
    diagnostics = sdk_steps[python_index + 1]
    generator = yaml_named_step(sdk_steps, "Install compatibility binding generator with telemetry", path)
    fallback_generator = yaml_named_step(sdk_steps, "Install compatibility binding generator", path)
    generator_index = sdk_steps.index(generator)
    if go_index >= generator_index:
        raise ContractFailure(f"binding contract check failed: SDK Go setup ordering ({path})")
    generator_steps = [step for step in sdk_steps if "uniffi-bindgen-go.git" in step]
    if generator_steps != [generator, fallback_generator]:
        raise ContractFailure(f"binding contract check failed: mutually exclusive compatibility generator installs ({path})")
    require_all(sdk, path, [
        ("SDK job waits for diagnostic tests and runner", r"^    needs:\s*\[\s*uniffi-diagnostics-tests\s*,\s*start-sdk-bindings-runner\s*\]\s*$"),
        ('SDK coverage and diagnostic test guard', "^    if:\\s*\\$\\{\\{\\s*always\\(\\)\\s*&&\\s*needs\\.uniffi-diagnostics-tests\\.result\\s*==\\s*'success'\\s*&&\\s*\\(\\s*\\(\\s*github\\.event_name\\s*==\\s*'pull_request'\\s*&&\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*!=\\s*github\\.repository\\s*\\|\\|\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*!=\\s*false\\s*\\|\\|\\s*github\\.actor\\s*==\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\)\\s*\\|\\|\\s*needs\\.start-sdk-bindings-runner\\.result\\s*==\\s*'success'\\s*\\)\\s*\\}\\}\\s*$"),
        ('SDK dynamic runner selection', "^    runs-on:\\s*\\$\\{\\{\\s*\\(\\s*github\\.event_name\\s*!=\\s*'pull_request'\\s*\\|\\|\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*==\\s*github\\.repository\\s*&&\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*==\\s*false\\s*&&\\s*github\\.actor\\s*!=\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\)\\s*&&\\s*needs\\.start-sdk-bindings-runner\\.result\\s*==\\s*'success'\\s*&&\\s*format\\(\\s*'cyclops-sdk-diag-\\{0\\}-\\{1\\}'\\s*,\\s*github\\.run_attempt\\s*,\\s*github\\.run_id\\s*\\)\\s*\\|\\|\\s*'ubuntu-24\\.04'\\s*\\}\\}\\s*$"),
    ])
    forbid(sdk, "SDK job EC2 OIDC permission", r"^      id-token:\s*write\s*$", path)
    require_all(diagnostics, path, [
        ("SDK diagnostic dependency step", r"^      - name:\s*Install UniFFI diagnostic dependencies\s*$"),
        ('trusted diagnostic dependency route', "^        if:\\s*\\$\\{\\{\\s*github\\.event_name\\s*!=\\s*'pull_request'\\s*\\|\\|\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*==\\s*github\\.repository\\s*&&\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*==\\s*false\\s*&&\\s*github\\.actor\\s*!=\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\}\\}\\s*$"),
        ("Pinned telemetry dependencies", r"python3\s+-m\s+pip\s+install\s+--disable-pip-version-check\s+-r\s+\.github/scripts/uniffi-diagnostics/requirements\.txt"),
    ])
    require_all(generator, path, [
        ('trusted telemetry generator route', "^        if:\\s*\\$\\{\\{\\s*github\\.event_name\\s*!=\\s*'pull_request'\\s*\\|\\|\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*==\\s*github\\.repository\\s*&&\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*==\\s*false\\s*&&\\s*github\\.actor\\s*!=\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\}\\}\\s*$"),
        ("Telemetry endpoint", r"OTEL_EXPORTER_OTLP_ENDPOINT:\s*https://otel\.cua\.ai"),
        ("Telemetry instance type", r"EC2_INSTANCE_TYPE:\s*\$\{\{\s*env\.EC2_RUNNER_INSTANCE_TYPE\s*\}\}"),
        ("Telemetry wrapper invocation", r"python3\s+\.github/scripts/uniffi-diagnostics/run_with_otel\.py\s*\\\s*--service-name\s+gha-uniffi-runner-diagnostic\s*\\\s*--heartbeat-seconds\s+30\s*\\\s*--sample-seconds\s+5\s*\\\s*--\s+env\s+CARGO_PROFILE_DEV_DEBUG=0\s+cargo\s+install\s+--debug"),
        ("Telemetry wrapper pinned generator", r"--git\s+https://github\.com/NordSecurity/uniffi-bindgen-go\.git\s*\\\s*--tag\s+['\"]v0\.7\.1\+v0\.31\.0['\"]\s*\\\s*--locked\s*\\\s*uniffi-bindgen-go"),
    ])
    require_all(fallback_generator, path, [
        ('untrusted hosted fallback route', "^        if:\\s*\\$\\{\\{\\s*github\\.event_name\\s*==\\s*'pull_request'\\s*&&\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*!=\\s*github\\.repository\\s*\\|\\|\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*!=\\s*false\\s*\\|\\|\\s*github\\.actor\\s*==\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\}\\}\\s*$"),
        ("hosted fallback plain generator command", r"CARGO_PROFILE_DEV_DEBUG=0\s+cargo\s+install\s+--debug\s*\\\s*--git\s+https://github\.com/NordSecurity/uniffi-bindgen-go\.git\s*\\\s*--tag\s+['\"]v0\.7\.1\+v0\.31\.0['\"]\s*\\\s*--locked\s*\\\s*uniffi-bindgen-go"),
    ])

    stop_permissions = yaml_mapping_block(stop, "permissions", 4, path)
    stop_steps = yaml_step_blocks(stop, path)
    stop_credentials = yaml_named_step(stop_steps, "Configure AWS credentials", path)
    terminate = yaml_named_step(stop_steps, "Terminate EC2 runner instances", path)
    require_all(stop, path, [
        ("EC2 stop dependencies", r"^    needs:\s*\[\s*start-sdk-bindings-runner\s*,\s*sdk-bindings\s*\]\s*$"),
        ("EC2 stop runner host", r"^    runs-on:\s*ubuntu-latest\s*$"),
        ('trusted unconditional EC2 cleanup', "^    if:\\s*\\$\\{\\{\\s*always\\(\\)\\s*&&\\s*\\(\\s*github\\.event_name\\s*!=\\s*'pull_request'\\s*\\|\\|\\s*\\(\\s*github\\.event\\.pull_request\\.head\\.repo\\.full_name\\s*==\\s*github\\.repository\\s*&&\\s*github\\.event\\.pull_request\\.head\\.repo\\.fork\\s*==\\s*false\\s*&&\\s*github\\.actor\\s*!=\\s*'dependabot\\[bot\\]'\\s*\\)\\s*\\)\\s*\\}\\}\\s*$"),
    ])
    require_all(stop_permissions, path, [
        ("EC2 stop OIDC permission", r"^      id-token:\s*write\s*$"),
    ])
    require_all(stop_credentials, path, [
        ("EC2 stop AWS credentials action", r"uses:\s*aws-actions/configure-aws-credentials@ff717079ee2060e4bcee96c4779b553acc87447c"),
        ("EC2 stop AWS role", r"role-to-assume:\s*arn:aws:iam::\$\{\{\s*env\.AWS_ACCOUNT_ID\s*\}\}:role/\$\{\{\s*env\.EC2_RUNNER_IAM_ROLE\s*\}\}"),
        ("EC2 stop AWS region", r"aws-region:\s*\$\{\{\s*env\.AWS_REGION\s*\}\}"),
    ])
    require_all(terminate, path, [
        ("Unconditional EC2 termination step", r"^        if:\s*always\(\)\s*$"),
        ("resolved EC2 instance output consumption", r"needs\.start-sdk-bindings-runner\.outputs\.ec2-instance-id"),
        ("EC2 cleanup fallback query", r"aws\s+ec2\s+describe-instances"),
        ("EC2 fallback run-id tag", r"Name=tag:github_job_id,Values=\$\{\{\s*github\.run_id\s*\}\}"),
        ("EC2 fallback repository tag", r"Name=tag:github_repo,Values=\$\{\{\s*github\.event\.repository\.name\s*\}\}"),
        ("EC2 fallback repository ID tag", r"Name=tag:github_repository_id,Values=\$\{\{\s*github\.repository_id\s*\}\}"),
        ("EC2 fallback run-attempt tag", r"Name=tag:github_run_attempt,Values=\$\{\{\s*github\.run_attempt\s*\}\}"),
        ("EC2 fallback diagnostic tag", r"Name=tag:uniffi_diagnostic,Values=true"),
        ("EC2 fallback nonterminated states", r"Name=instance-state-name,Values=pending,running,shutting-down,stopping,stopped"),
        ("EC2 fallback empty result normalization", r"if\s+\[\s+\"\$MATCHING_INSTANCE_IDS\"\s*=\s*\"None\"\s+\];\s*then\s+MATCHING_INSTANCE_IDS=\"\""),
        ("EC2 multi-ID splitting", r"tr\s+'\\t\s+'\s+'\\n'"),
        ("EC2 terminate instance command", r"aws\s+ec2\s+terminate-instances\s*\\\s*--instance-ids\s+\"\$\{INSTANCE_IDS\[@\]\}\"\s*\\\s*--region\s+\"\$\{\{\s*env\.AWS_REGION\s*\}\}"),
    ])

def require_lifecycle(source: str, path: Path, label: str, queue_marker: str, calls: list[str], exhausted: str) -> None:
    checks = [(f"{label} request queue", queue_marker)]
    checks.extend((f"{label} lifecycle {call}", call) for call in calls)
    checks.append((f"{label} queue exhaustion", exhausted))
    require_all(source, path, checks)


def require_python_namespace_lifecycle_contract(path: Path) -> None:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=path)
    functions = {
        node.name: node
        for node in tree.body
        if isinstance(node, (ast.AsyncFunctionDef, ast.FunctionDef))
    }

    expected_lifecycle = functions.get("expected_lifecycle")
    run_lifecycle = functions.get("run_lifecycle")
    if expected_lifecycle is None or run_lifecycle is None:
        raise ContractFailure(f"binding contract check failed: Python namespace lifecycle functions ({path})")

    return_statement = next(
        (node for node in expected_lifecycle.body if isinstance(node, ast.Return)),
        None,
    )
    if not isinstance(return_statement, ast.Return) or not isinstance(return_statement.value, ast.List):
        raise ContractFailure(f"binding contract check failed: Python namespace lifecycle request queue ({path})")

    requests = []
    for item in return_statement.value.elts:
        if not isinstance(item, ast.Call) or not isinstance(item.func, ast.Name) or item.func.id != "Expected":
            continue
        if len(item.args) < 5:
            raise ContractFailure(f"binding contract check failed: Python lifecycle Expected fields ({path})")
        method = item.args[0].value if isinstance(item.args[0], ast.Constant) else None
        status = item.args[4].value if isinstance(item.args[4], ast.Constant) else None
        requests.append((method, ast.unparse(item.args[1]), status))

    required_requests = [
        ("namespace create", "POST", "f'{BASE}/api/namespaces'", 201),
        ("namespace get", "GET", "f'{BASE}/api/namespaces/default'", 200),
        ("pool namespace compatibility create", "POST", "f'{BASE}/api/namespaces'", 409),
        ("pool create", "POST", "pool_url.removesuffix('/default')", 201),
        ("pool delete", "DELETE", "pool_url", 204),
        ("pool namespace cleanup", "DELETE", "f'{BASE}/api/namespaces/default'", 204),
        ("explicit namespace delete", "DELETE", "f'{BASE}/api/namespaces/default'", 404),
    ]
    request_index = -1
    for label, method, url, status in required_requests:
        request_index = next(
            (
                index
                for index in range(request_index + 1, len(requests))
                if requests[index] == (method, url, status)
            ),
            None,
        )
        if request_index is None:
            raise ContractFailure(f"binding contract check failed: Python {label} request ordering/status ({path})")

    lifecycle_calls = [
        node.value.func.attr
        for node in sorted(ast.walk(run_lifecycle), key=lambda node: (getattr(node, "lineno", 0), getattr(node, "col_offset", 0)))
        if isinstance(node, ast.Await)
        and isinstance(node.value, ast.Call)
        and isinstance(node.value.func, ast.Attribute)
        and isinstance(node.value.func.value, ast.Name)
        and node.value.func.value.id == "sdk"
    ]
    required_calls = [
        "create_namespace",
        "get_namespace",
        "create_pool",
        "delete_pool",
        "delete_namespace",
    ]
    call_index = -1
    for call in required_calls:
        call_index = next(
            (index for index in range(call_index + 1, len(lifecycle_calls)) if lifecycle_calls[index] == call),
            None,
        )
        if call_index is None:
            raise ContractFailure(f"binding contract check failed: Python lifecycle {call} ordering ({path})")


def check(root: Path) -> None:
    bindings = root / "cyclops-cs/sdk-bindings"
    files = {
        "kotlin_test": bindings / "kotlin/tests/TestAsyncClient.kt",
        "kotlin_example": bindings / "examples/kotlin/AppControlled.kt",
        "kotlin_live": bindings / "examples/kotlin/LiveAppControlled.kt",
        "swift_test": bindings / "swift/tests/TestAsyncClient.swift",
        "swift_example": bindings / "examples/swift/AppControlled.swift",
        "swift_live": bindings / "examples/swift/LiveAppControlled.swift",
        "ruby_test": bindings / "ruby/tests/test_async_client.rb",
        "ruby_example": bindings / "examples/ruby/app_controlled.rb",
        "ruby_live": bindings / "examples/ruby/live_app_controlled.rb",
        "python_test": bindings / "python/tests/test_async_client.py",
        "python_fixture": bindings / "python/contract_fixture.py",
        "python_example": bindings / "examples/python/app_controlled.py",
        "python_live": bindings / "examples/python/live_app_controlled.py",
        "python_sdk": bindings / "python/fleet_sdk/_sdk.py",
        "kotlin_sdk": bindings / "kotlin/ai/cua/cyclops/sdk/fleet_sdk.kt",
        "swift_sdk": bindings / "swift/CyclopsSdk.swift",
        "ruby_sdk": bindings / "ruby/cyclops_sdk/sdk.rb",
        "kotlin_build": bindings / "kotlin/build.gradle.kts",
        "swift_runner": root / "cyclops-cs/scripts/run-swift-sdk-binding.sh",
        "ruby_runner": root / "cyclops-cs/scripts/run-ruby-sdk-binding.sh",
        "compat_generator": root / "cyclops-cs/scripts/generate-compat-sdk-bindings.sh",
        "sdk_bindings_workflow": root / ".github/workflows/ci-cyclops-cs.yml",
    }
    source = {name: executable_source(path) for name, path in files.items()}

    require_all(source["kotlin_build"], files["kotlin_build"], KOTLIN_BUILD_CHECKS)
    require_all(source["swift_runner"], files["swift_runner"], [
        ("Swift runner module name", r"-module-name\s+CyclopsSdk"),
        ("Swift runner Swift include directory", r"-I\s+\"\$swift_dir\""),
        ("Swift runner SDK FFI module map", r"-Xcc\s+\"-fmodule-map-file=\$swift_dir/CyclopsSdkFFI\.modulemap\""),
        ("Swift runner schema FFI module map", r"-Xcc\s+\"-fmodule-map-file=\$swift_dir/CyclopsSdkSchemaFFI\.modulemap\""),
        ("Swift runner generated SDK source", r"\"\$swift_dir/CyclopsSdk\.swift\""),
        ("Swift runner generated schema source", r"\"\$swift_dir/CyclopsSdkSchema\.swift\""),
        ("Swift runner library rpath", r"-Xlinker\s+-rpath\s+-Xlinker\s+\"\$library_dir\""),
        ("Swift runner array-safe invocation", r"swiftc\s+\"\$\{swiftc_args\[@\]\}\""),
    ])
    require_all(source["ruby_runner"], files["ruby_runner"], [
        ("Ruby runner native library directory", r"library_dir=\"\$runtime/cyclops_sdk\""),
        ("Ruby runner Linux loader path", r"LD_LIBRARY_PATH=\"\$library_dir\$\{LD_LIBRARY_PATH:\+:[^}]+\}\""),
        ("Ruby runner macOS loader path", r"DYLD_LIBRARY_PATH=\"\$library_dir\$\{DYLD_LIBRARY_PATH:\+:[^}]+\}\""),
        ("Ruby runner Windows loader path", r"PATH=\"\$library_dir:\$PATH\""),
    ])

    require_all(source["compat_generator"], files["compat_generator"], [
        ("Compatibility generator gofmt prerequisite", r"if ! command -v gofmt >/dev/null 2>&1; then"),
        ("Compatibility generator gofmt guidance", r"gofmt is required to normalize compatibility Go bindings"),
    ])
    require_sdk_bindings_workflow(files["sdk_bindings_workflow"])

    require_all(source["kotlin_live"], files["kotlin_live"], [
        ("Kotlin live HTTP callback", r"class\s+JavaHttpClient\s*:\s*HttpClient"),
        ("Kotlin live typed pool", r"OsGymSandboxWarmPoolSpec\(.*SandboxTemplateRef\(templateName\)"),
        ("Kotlin live typed template", r"CreateTemplateRequest\(namespace,\s*templateName,"),
        ("Kotlin live template service", r"SandboxService\(\"mcp\",\s*3000u"),
        ("Kotlin live typed claim", r"CreateClaimRequest\(pool,\s*null\)"),
        ("Kotlin live MCP retry", r"response\.status\.toInt\(\)\s+in\s+setOf\(502,\s*503,\s*504\)"),
        ("Kotlin live cleanup", r"client\.deleteClaim.*client\.deletePool"),
    ])

    require_all(source["kotlin_test"], files["kotlin_test"], [
        ("Kotlin synchronized scripted callback", r"private\s+val\s+lock\s*=\s*Any\(\)"),
        ("Kotlin request headers comparison", r"request\.headers\.map\s*\{\s*it\.name\s+to\s+it\.value\s*}\s*==\s*item\.headers"),
        ("Kotlin optional binary body comparison", r"request\.body\.contentEquals\(item\.body\)"),
        ("Kotlin callback transport constructor", r"throw\s+HttpException\.Transport\("),
        ("Kotlin SDK transport assertion", r"catch\s*\(\s*error\s*:\s*SdkException\.Transport\s*\)"),
        ("Kotlin concurrent SDK calls", r"async\s*\{\s*concurrentClient\.serviceRequest"),
    ])
    require_all(source["swift_live"], files["swift_live"], [
        ("Swift live URLSession callback", r"final\s+class\s+UrlSessionHttpClient\s*:\s*HttpClient"),
        ("Swift live typed pool", r"OsGymSandboxWarmPoolSpec\(replicas:\s*1,\s*sandboxTemplateRef:"),
        ("Swift live typed template", r"CreateTemplateRequest\(namespace:\s*namespace,\s*name:\s*templateName"),
        ("Swift live template service", r"SandboxService\(name:\s*\"mcp\",\s*targetPort:\s*3000"),
        ("Swift live typed claim", r"CreateClaimRequest\(pool:\s*pool,\s*spec:\s*nil\)"),
        ("Swift live MCP retry", r"\[502,\s*503,\s*504\]\.contains\(Int\(response\.status\)\)"),
        ("Swift live portable duration", r"advanced\(by:\s*\.seconds\(300\)\)"),
        ("Swift live cleanup", r"client\.deleteClaim.*client\.deletePool"),
    ])

    require_all(source["swift_test"], files["swift_test"], [
        ("Swift actor callback", r"actor\s+ScriptedHttpClient\s*:\s*HttpClient"),
        ("Swift generated callback label", r"func\s+execute\(request:\s*HttpRequest\)\s+async\s+throws\s*->\s*HttpResponse"),
        ("Swift exact headers comparison", r"request\.headers\s*==\s*item\.headers"),
        ("Swift optional Data body comparison", r"request\.body\s*==\s*item\.body"),
        ("Swift callback transport constructor", r"throw\s+HttpError\.Transport\("),
        ("Swift SDK transport assertion", r"catch\s+SdkError\.Transport\(let\s+reason\)"),
        ("Swift concurrent SDK calls", r"async\s+let\s+first\s*=\s*concurrentClient\.serviceRequest"),
    ])
    require_all(source["ruby_live"], files["ruby_live"], [
        ("Ruby live Net HTTP callback", r"class\s+NetHttpClient\s*<\s*FleetSdk::HttpClient"),
        ("Ruby live typed pool", r"FleetSdk::OSGymSandboxWarmPoolSpec\.new\(replicas:\s*1,\s*sandbox_template_ref:"),
        ("Ruby live typed template", r"FleetSdk::CreateTemplateRequest\.new\(namespace:\s*namespace,\s*name:\s*template_name"),
        ("Ruby live template service", r"FleetSdk::SandboxService\.new\(name:\s*\x27mcp\x27,\s*target_port:\s*3000"),
        ("Ruby live typed claim", r"CreateClaimRequest\.new\(pool:\s*pool,\s*spec:\s*nil\)"),
        ("Ruby live MCP retry", r"\[502,\s*503,\s*504\]\.include\?\(response\.status\)"),
        ("Ruby live cleanup", r"client\.delete_claim.*client\.delete_pool"),
    ])
    forbid(source["ruby_live"], "Ruby live byte arrays", r"\.bytes\b", files["ruby_live"])
    forbid(source["ruby_live"], "Ruby live future wrapper", r"\)\.value\b", files["ruby_live"])

    require_all(source["ruby_test"], files["ruby_test"], [
        ("Ruby mutex-safe callback", r"@mutex\s*=\s*Mutex\.new"),
        ("Ruby mutex queue access", r"@mutex\.synchronize\s+do"),
        ("Ruby exact headers and body comparison", r"actual\s*==\s*\[item\.method,\s*item\.url,\s*item\.headers,\s*item\.body\]"),
        ("Ruby callback transport constructor", r"raise\s+FleetSdk::HttpError::Transport\.new\("),
        ("Ruby future transport assertion", r"rescue\s+FleetSdk::SdkError::Transport\s*=>\s*error"),
        ("Ruby concurrent SDK threads", r"Thread\.new\s+do"),
    ])
    require_all(source["python_test"], files["python_test"], [
        ("Python native body queue", r"expected_service_calls\("),
        ("Python absent body assertion", r"assertIsNone\(transport\.requests\[1\]\.body\)"),
        ("Python binary response assertion", r"response\.body\s+for\s+response\s+in\s+responses"),
        ("Python one-client concurrent SDK calls", r"asyncio\.gather\(\s*sdk\.service_request"),
        ("Python one-client queue recording", r"len\(transport\.requests\)"),
        ("Python concurrent queue exhaustion", r"transport\.assert_exhausted\(\)"),
    ])
    require_all(source["python_fixture"], files["python_fixture"], [
        ("Python asyncio callback lock", r"self\.lock\s*=\s*asyncio\.Lock\(\)"),
        ("Python locked callback", r"async\s+with\s+self\.lock"),
        ("Python exact headers and body comparison", r"actual_headers.*request\.body.*item\.headers.*item\.body"),
        ("Python service call queue", r"def\s+expected_service_calls\("),
        ("Python absent optional body helper", r"def\s+service_request\(body\)"),
        ("Python namespace create", r"await\s+sdk\.create_namespace\("),
        ("Python namespace get", r"await\s+sdk\.get_namespace\("),
        ("Python namespace delete", r"await\s+sdk\.delete_namespace\("),
    ])
    require_python_namespace_lifecycle_contract(files["python_fixture"])
    require_all(source["python_sdk"], files["python_sdk"], [
        ("Python create namespace binding", r"async def create_namespace\(self, name: str\) -> Namespace"),
        ("Python get namespace binding", r"async def get_namespace\(self, name: str\) -> Namespace"),
        ("Python delete namespace binding", r"async def delete_namespace\(self, name: str\) -> None"),
    ])
    require_all(source["kotlin_sdk"], files["kotlin_sdk"], [
        ("Kotlin create namespace binding", r"suspend\s+fun\s+`createNamespace`\(`name`:\s*kotlin\.String\):\s*Namespace"),
        ("Kotlin get namespace binding", r"suspend\s+fun\s+`getNamespace`\(`name`:\s*kotlin\.String\):\s*Namespace"),
        ("Kotlin delete namespace binding", r"suspend\s+fun\s+`deleteNamespace`\(`name`:\s*kotlin\.String\)"),
    ])
    require_all(source["swift_sdk"], files["swift_sdk"], [
        ("Swift create namespace binding", r"func\s+createNamespace\(name:\s*String\)\s+async\s+throws\s*->\s*Namespace"),
        ("Swift get namespace binding", r"func\s+getNamespace\(name:\s*String\)\s+async\s+throws\s*->\s*Namespace"),
        ("Swift delete namespace binding", r"func\s+deleteNamespace\(name:\s*String\)\s+async\s+throws"),
    ])
    require_all(source["ruby_sdk"], files["ruby_sdk"], [
        ("Ruby create namespace binding", r"def\s+create_namespace\(name\)"),
        ("Ruby get namespace binding", r"def\s+get_namespace\(name\)"),
        ("Ruby delete namespace binding", r"def\s+delete_namespace\(name\)"),
    ])
    require(files["python_live"].read_text(encoding="utf-8"), "Python live environment", r"os\.environ\[\"CUA_CLIENT_ID\"\].*os\.environ\[\"CUA_IMAGE\"\]", files["python_live"])
    require_all(source["python_live"], files["python_live"], [
        ("Python live native HTTP constructor", r"CyclopsClient\.connect_with_native_http_client\("),
        ("Python live typed pool", r"OsGymSandboxWarmPoolSpec\(.*sandbox_template_ref"),
        ("Python live typed template", r"CreateTemplateRequest\("),
        ("Python live template service", r"SandboxService\(.*target_port=3000"),
        ("Python live typed claim", r"CreateClaimRequest\(pool=pool,\s*spec=None\)"),
        ("Python live MCP retry", r"response\.status\s+in\s+\(502,\s*503,\s*504\)"),
        ("Python live cleanup", r"await\s+client\.delete_claim.*await\s+client\.delete_pool"),
    ])

    lifecycle_calls = {
        "kotlin_test": (r"lifecycleQueue\(", [r"createPool\(", r"createClaim\(", r"waitClaim\(", r"serviceRequest\(", r"deleteClaim\(", r"deletePool\("], r"transport\.assertExhausted\("),
        "kotlin_example": (r"Expected\(", [r"createPool\(", r"createClaim\(", r"waitClaim\(", r"serviceRequest\(", r"deleteClaim\(", r"deletePool\("], r"transport\.assertExhausted\("),
        "swift_test": (r"lifecycleQueue\(", [r"createPool\(", r"createClaim\(", r"waitClaim\(", r"serviceRequest\(", r"deleteClaim\(", r"deletePool\("], r"transport\.assertExhausted\("),
        "swift_example": (r"private\s+var\s+expected", [r"createPool\(", r"createClaim\(", r"waitClaim\(", r"serviceRequest\(", r"deleteClaim\(", r"deletePool\("], r"transport\.assertExhausted\("),
        "ruby_test": (r"Expected\.new", [r"create_pool\(", r"create_claim\(", r"wait_claim\(", r"service_request\(", r"delete_claim\(", r"delete_pool\("], r"transport\.assert_exhausted!"),
        "ruby_example": (r"Expected\.new", [r"create_pool\(", r"create_claim\(", r"wait_claim\(", r"service_request\(", r"delete_claim\(", r"delete_pool\("], r"transport\.assert_exhausted!"),
        "python_fixture": (r"def\s+expected_lifecycle\(", [r"create_pool\(", r"create_claim\(", r"wait_claim\(", r"service_request\(", r"delete_claim\(", r"delete_pool\("], r"transport\.assert_exhausted\("),
        "python_test": (r"expected_lifecycle\(", [r"run_lifecycle\(", r"ScriptedHttpClient\("], r"transport\.assert_exhausted\("),
        "python_example": (r"expected_lifecycle\(", [r"run_lifecycle\(", r"ScriptedHttpClient\("], r"run_lifecycle\("),
    }
    for name, (queue, calls, exhausted) in lifecycle_calls.items():
        require_lifecycle(source[name], files[name], name.replace("_", " "), queue, calls, exhausted)

    for name in ("ruby_test", "ruby_example"):
        require_all(source[name], files[name], [
            ("Ruby anchored pool collection route", r"pool_url\.sub\(%r\{/default\\z\}"),
            ("Ruby anchored claim collection route", r"claim_url\.sub\(%r\{/default\\z\}"),
        ])
        forbid(source[name], "Ruby byte arrays instead of binary strings", r"\.bytes\b", files[name])
        forbid(source[name], "Ruby future wrapper on resolved async result", r"\)\.value\b", files[name])


def kotlin_build_fixture() -> str:
    return """\
sourceSets.test { kotlin.srcDirs(\"tests\") }
val example by sourceSets.creating {
    kotlin.srcDir(\"../examples/kotlin\")
    compileClasspath += sourceSets.main.get().output
    runtimeClasspath += sourceSets.main.get().output
}
configurations[example.implementationConfigurationName].extendsFrom(
    configurations[sourceSets.main.get().implementationConfigurationName],
)
configurations[example.runtimeOnlyConfigurationName].extendsFrom(
    configurations[sourceSets.main.get().runtimeOnlyConfigurationName],
)
tasks.register<JavaExec>(\"example\") {
    dependsOn(example.classesTaskName)
    classpath = example.runtimeClasspath
}
"""


def sdk_bindings_workflow_fixture() -> str:
    return """on:
  push:
    paths:
      - ".github/scripts/uniffi-diagnostics/**"
      - ".github/scripts/tests/test_run_with_otel.py"
  pull_request:
    paths:
      - ".github/scripts/uniffi-diagnostics/**"
      - ".github/scripts/tests/test_run_with_otel.py"
permissions:
  contents: read
env:
  AWS_REGION: us-west-2
  AWS_ACCOUNT_ID: "296062593712"
  EC2_RUNNER_IAM_ROLE: github-actions-nixos-ami
  EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge
  EC2_RUNNER_SUBNET_ID: subnet-0ca86dea50cd84d24
  EC2_RUNNER_SECURITY_GROUP_ID: sg-078f4e7b96883e60e
jobs:
  uniffi-diagnostics-tests:
    name: UniFFI diagnostic wrapper tests
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5
      - name: Set up Python
        uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065
        with:
          python-version: "3.11"
      - name: Run UniFFI diagnostic wrapper tests
        run: |
          python3 -m pip install --disable-pip-version-check -r .github/scripts/uniffi-diagnostics/requirements.txt
          python3 .github/scripts/tests/test_run_with_otel.py -v
          cyclops-cs/scripts/check-sdk-binding-contract-sources.sh
          cyclops-cs/scripts/check-sdk-binding-contract-sources.sh --self-test
  start-sdk-bindings-runner:
    name: Start UniFFI EC2 Runner
    needs: uniffi-diagnostics-tests
    if: ${{ github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]') }}
    permissions:
      id-token: write
    runs-on: ubuntu-latest
    outputs:
      ec2-instance-id: ${{ steps.resolve-ec2.outputs.ec2-instance-id }}
    steps:
      - name: Configure AWS credentials
        uses: aws-actions/configure-aws-credentials@ff717079ee2060e4bcee96c4779b553acc87447c
        with:
          role-to-assume: arn:aws:iam::${{ env.AWS_ACCOUNT_ID }}:role/${{ env.EC2_RUNNER_IAM_ROLE }}
          aws-region: ${{ env.AWS_REGION }}
      - name: Get latest Ubuntu 24.04 AMI
        id: ami
        run: |
          AMI_ID=$(aws ec2 describe-images \\
            --region "${{ env.AWS_REGION }}" \\
            --owners 099720109477 \\
            --filters "Name=name,Values=ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*" \\
            --query 'sort_by(Images, &CreationDate)[-1].ImageId' \\
            --output text)
          test -n "$AMI_ID"
          test "$AMI_ID" != "None"
          echo "ami_id=$AMI_ID" >> "$GITHUB_OUTPUT"
      - name: Start EC2 runner
        id: start-ec2
        uses: unblocked/ec2-action-builder@504a3cf746c6090065a66896a3addc2bb101b4ba
        with:
          github_token: ${{ secrets.QEMU_BUILD_GH_PERSONAL_ACCESS_TOKEN }}
          aws_access_key_id: ${{ env.AWS_ACCESS_KEY_ID }}
          aws_secret_access_key: ${{ env.AWS_SECRET_ACCESS_KEY }}
          aws_session_token: ${{ env.AWS_SESSION_TOKEN }}
          aws_region: ${{ env.AWS_REGION }}
          ec2_subnet_id: ${{ env.EC2_RUNNER_SUBNET_ID }}
          ec2_security_group_id: ${{ env.EC2_RUNNER_SECURITY_GROUP_ID }}
          ec2_instance_type: ${{ env.EC2_RUNNER_INSTANCE_TYPE }}
          ec2_ami_id: ${{ steps.ami.outputs.ami_id }}
          ec2_root_disk_size_gb: "60"
          ec2_spot_instance_strategy: None
          ec2_instance_ttl: 60
          ec2_instance_tags: >-
            [{"Key":"github_run_attempt","Value":"${{ github.run_attempt }}"},{"Key":"github_repository_id","Value":"${{ github.repository_id }}"},{"Key":"uniffi_diagnostic","Value":"true"}]
          github_action_runner_version: "2.333.1"
          github_action_runner_label_prefix: cyclops-sdk-diag-${{ github.run_attempt }}-
      - name: Resolve EC2 runner instance
        id: resolve-ec2
        if: always()
        run: |
          INSTANCE_ID="$(aws ec2 describe-instances \\
            --region "${{ env.AWS_REGION }}" \\
            --filters \\
              "Name=tag:github_job_id,Values=${{ github.run_id }}" \\
              "Name=tag:github_repo,Values=${{ github.event.repository.name }}" \\
              "Name=tag:github_repository_id,Values=${{ github.repository_id }}" \\
              "Name=tag:github_run_attempt,Values=${{ github.run_attempt }}" \\
              "Name=tag:uniffi_diagnostic,Values=true" \\
            --query 'Reservations[].Instances[].InstanceId' \\
            --output text)"
          if [ "$INSTANCE_ID" = "None" ]; then
            INSTANCE_ID=""
          fi
          echo "ec2-instance-id=$INSTANCE_ID" >> "$GITHUB_OUTPUT"
  sdk-bindings:
    name: UniFFI SDK bindings (Linux)
    needs: [uniffi-diagnostics-tests, start-sdk-bindings-runner]
    if: ${{ always() && needs.uniffi-diagnostics-tests.result == 'success' && ((github.event_name == 'pull_request' && (github.event.pull_request.head.repo.full_name != github.repository || github.event.pull_request.head.repo.fork != false || github.actor == 'dependabot[bot]')) || needs.start-sdk-bindings-runner.result == 'success') }}
    runs-on: ${{ (github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]')) && needs.start-sdk-bindings-runner.result == 'success' && format('cyclops-sdk-diag-{0}-{1}', github.run_attempt, github.run_id) || 'ubuntu-24.04' }}
    timeout-minutes: 45
    steps:
      - name: Checkout code
        uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5
      - name: Install EC2 runner prerequisites
        if: ${{ runner.environment == 'self-hosted' }}
        run: |
          sudo apt-get update
          sudo apt-get install --yes unzip
      - name: Setup Go
        uses: actions/setup-go@d35c59abb061a4a6fb18e82ac0862c26744d6ab5
        with:
          go-version-file: cyclops-cs/backend/go.mod
      - name: Set up Node.js
        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version-file: cyclops-cs/.nvmrc
      - name: Set up Gradle
        uses: gradle/actions/setup-gradle@0b6dd653ba04f4f93bf581ec31e66cbd7dcb644d
      - name: Set up Python
        uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065
      - name: Install UniFFI diagnostic dependencies
        if: ${{ github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]') }}
        run: python3 -m pip install --disable-pip-version-check -r .github/scripts/uniffi-diagnostics/requirements.txt
      - name: Install compatibility binding generator with telemetry
        if: ${{ github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]') }}
        env:
          OTEL_EXPORTER_OTLP_ENDPOINT: https://otel.cua.ai
          EC2_INSTANCE_TYPE: ${{ env.EC2_RUNNER_INSTANCE_TYPE }}
        run: |
          python3 .github/scripts/uniffi-diagnostics/run_with_otel.py \\
            --service-name gha-uniffi-runner-diagnostic \\
            --heartbeat-seconds 30 \\
            --sample-seconds 5 \\
            -- env CARGO_PROFILE_DEV_DEBUG=0 cargo install --debug \\
              --git https://github.com/NordSecurity/uniffi-bindgen-go.git \\
              --tag 'v0.7.1+v0.31.0' \\
              --locked \\
              uniffi-bindgen-go
      - name: Install compatibility binding generator
        if: ${{ github.event_name == 'pull_request' && (github.event.pull_request.head.repo.full_name != github.repository || github.event.pull_request.head.repo.fork != false || github.actor == 'dependabot[bot]') }}
        run: |
          CARGO_PROFILE_DEV_DEBUG=0 cargo install --debug \\
            --git https://github.com/NordSecurity/uniffi-bindgen-go.git \\
            --tag 'v0.7.1+v0.31.0' \\
            --locked \\
            uniffi-bindgen-go
      - name: Check generated bindings and generator regression harness
        run: cyclops-cs/scripts/test-generate-sdk-bindings.sh
  stop-sdk-bindings-runner:
    name: Terminate UniFFI EC2 Runner
    needs: [start-sdk-bindings-runner, sdk-bindings]
    runs-on: ubuntu-latest
    if: ${{ always() && (github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]')) }}
    permissions:
      id-token: write
    steps:
      - name: Configure AWS credentials
        uses: aws-actions/configure-aws-credentials@ff717079ee2060e4bcee96c4779b553acc87447c
        with:
          role-to-assume: arn:aws:iam::${{ env.AWS_ACCOUNT_ID }}:role/${{ env.EC2_RUNNER_IAM_ROLE }}
          aws-region: ${{ env.AWS_REGION }}
      - name: Terminate EC2 runner instances
        if: always()
        env:
          RESOLVED_INSTANCE_ID: ${{ needs.start-sdk-bindings-runner.outputs.ec2-instance-id }}
        run: |
          MATCHING_INSTANCE_IDS="$(aws ec2 describe-instances \\
            --region "${{ env.AWS_REGION }}" \\
            --filters \\
              "Name=tag:github_job_id,Values=${{ github.run_id }}" \\
              "Name=tag:github_repo,Values=${{ github.event.repository.name }}" \\
              "Name=tag:github_repository_id,Values=${{ github.repository_id }}" \\
              "Name=tag:github_run_attempt,Values=${{ github.run_attempt }}" \\
              "Name=tag:uniffi_diagnostic,Values=true" \\
              "Name=instance-state-name,Values=pending,running,shutting-down,stopping,stopped" \\
            --query 'Reservations[].Instances[].InstanceId' \\
            --output text)"
          if [ "$MATCHING_INSTANCE_IDS" = "None" ]; then
            MATCHING_INSTANCE_IDS=""
          fi
          mapfile -t INSTANCE_IDS < <(printf '%s\\n' "$RESOLVED_INSTANCE_ID" "$MATCHING_INSTANCE_IDS" | tr '\\t ' '\\n' | awk 'NF && !seen[$0]++')
          if ((${#INSTANCE_IDS[@]})); then
            aws ec2 terminate-instances \\
              --instance-ids "${INSTANCE_IDS[@]}" \\
              --region "${{ env.AWS_REGION }}"
          else
            echo "No matching EC2 instances found"
          fi
"""

def validate_yaml_fixture(workflow: str) -> None:
    try:
        import yaml
    except ImportError:
        first = next((line for line in workflow.splitlines() if line.strip()), "")
        if first not in {"on:", "permissions:"} or workflow.startswith("\\"):
            raise ContractFailure("workflow fixture has an invalid prefix")
        return

    try:
        yaml.safe_load(workflow)
    except yaml.YAMLError as error:
        raise ContractFailure(f"workflow fixture is not valid YAML: {error}") from error


def require_workflow_rejected(path: Path, workflow: str, description: str) -> None:
    validate_yaml_fixture(workflow)
    path.write_text(workflow, encoding="utf-8")
    try:
        require_sdk_bindings_workflow(path)
    except ContractFailure:  # lint-ignore: swallowed-exception
        return
    raise ContractFailure(f"{description} unexpectedly passed")


def self_test() -> None:
    samples = {
        "kotlin": (strip_c_like_comments, "/* throw HttpException.Transport() */\n", r"HttpException\.Transport"),
        "swift": (strip_c_like_comments, "// actor ScriptedHttpClient: HttpClient\n", r"actor\s+ScriptedHttpClient"),
        "ruby": (strip_ruby_comments, "=begin\nThread.new do\n=end\n# Mutex.new\n", r"Thread\.new|Mutex\.new"),
        "python": (strip_python_comments_and_strings, '\"\"\" asyncio.gather(sdk.service_request()) \"\"\"\n# expected_service_calls()\n', r"asyncio\.gather|expected_service_calls"),
        "yaml": (strip_yaml_comments, "# ec2_instance_type: m7i.2xlarge\n", r"ec2_instance_type"),
    }
    for language, (stripper, comment_only, pattern) in samples.items():
        if re.search(pattern, stripper(comment_only), flags=re.DOTALL):
            raise ContractFailure(f"comment-only negative self-test failed for {language}")

    quoted_yaml = strip_yaml_comments(
        "SINGLE: 'it''s first\n  # retained-single\n  final' # removed-single\n"
        'DOUBLE: "first \"quoted\"\n  # retained-double\n  final" # removed-double\n'
    )
    if (
        "# retained-single" not in quoted_yaml
        or "# retained-double" not in quoted_yaml
        or "# removed-single" in quoted_yaml
        or "# removed-double" in quoted_yaml
    ):
        raise ContractFailure("YAML multiline quoted-scalar comment stripping self-test failed")

    matrix = [
        ("trusted same-repository PR", "pull_request", False, "cloud", "cloud", "member", "success", True, "cyclops-sdk-diag-2-100"),
        ("untrusted fork PR", "pull_request", True, "fork", "cloud", "contributor", "skipped", True, "ubuntu-24.04"),
        ("Dependabot PR", "pull_request", False, "cloud", "cloud", "dependabot[bot]", "skipped", True, "ubuntu-24.04"),
    ]
    for description, event_name, is_fork, head_repository, repository, actor, start_result, expected_runs, expected_runner in matrix:
        trusted = is_trusted_uniffi_ec2_event(event_name, is_fork, head_repository, repository, actor)
        runs, runner = sdk_bindings_route(trusted, start_result, "2", "100")
        if (runs, runner) != (expected_runs, expected_runner):
            raise ContractFailure(f"SDK runner routing matrix failed for {description}")
    if sdk_bindings_route(True, "failure", "2", "100") != (False, None):
        raise ContractFailure("trusted EC2 start failure unexpectedly falls back")

    with tempfile.TemporaryDirectory() as directory:
        fixture = Path(directory) / "build.gradle.kts"
        fixture.write_text(kotlin_build_fixture(), encoding="utf-8")
        require_kotlin_build(fixture)

        wiring = kotlin_build_fixture().splitlines()
        fixture.write_text(
            "// " + wiring[0] + "\n"
            "/* outer Gradle comment\n"
            "  /* nested Gradle comment\n"
            + "\n".join("    " + line for line in wiring[1:-1])
            + "\n  */\n"
            "*/\n"
            "// " + wiring[-1] + "\n",
            encoding="utf-8",
        )
        try:
            require_kotlin_build(fixture)
        except ContractFailure:  # lint-ignore: swallowed-exception
            pass
        else:
            raise ContractFailure("comment-only .kts Gradle wiring unexpectedly passed")

        fixture = Path(directory) / "ci-cyclops-cs.yml"
        workflow = sdk_bindings_workflow_fixture()
        validate_yaml_fixture(workflow)
        first_line = next(line for line in workflow.splitlines() if line.strip())
        if first_line not in {"on:", "permissions:"} or workflow.startswith("\\"):
            raise ContractFailure("workflow fixture has a stray prefix")
        fixture.write_text(workflow, encoding="utf-8")
        require_sdk_bindings_workflow(fixture)

        require_workflow_rejected(
            fixture,
            workflow.replace(
                "  EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge",
                "  # EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge",
            ),
            "comment-only EC2 workflow requirement",
        )

        require_workflow_rejected(
            fixture,
            workflow.replace(
                '          ec2_root_disk_size_gb: "60"',
                '          # ec2_root_disk_size_gb: "60"',
            )
            + """\
  unrelated:
    runs-on: ubuntu-latest
    steps:
      - name: Misplaced EC2 setting
        uses: unblocked/ec2-action-builder@504a3cf746c6090065a66896a3addc2bb101b4ba
        with:
          ec2_root_disk_size_gb: "60"
""",
            "unrelated-job EC2 workflow requirement",
        )

        require_workflow_rejected(
            fixture,
            workflow.replace(
                "      - name: Install compatibility binding generator with telemetry",
                "      - name: Install compatibility binding generator\n"
                "        run: CARGO_PROFILE_DEV_DEBUG=0 cargo install --debug --git https://github.com/NordSecurity/uniffi-bindgen-go.git --tag 'v0.7.1+v0.31.0' --locked uniffi-bindgen-go\n"
                "      - name: Install compatibility binding generator with telemetry",
            ).replace(
                "            -- env CARGO_PROFILE_DEV_DEBUG=0 cargo install --debug \\\n",
                "            -- ./generate.sh\n",
            ),
            "split wrapper and compatibility generator install",
        )

        require_workflow_rejected(
            fixture,
            workflow.replace(
                "  EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge\n",
                '  UNRELATED: "quoted # EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge"\n',
            ),
            "quoted-scalar EC2 workflow injection",
        )

        for mutated_workflow, description in [
            (
                workflow.replace("          aws-region: ${{ env.AWS_REGION }}\n", "", 1),
                "start AWS region requirement",
            ),
            (
                workflow.rsplit("          aws-region: ${{ env.AWS_REGION }}\n", 1)[0]
                + workflow.rsplit("          aws-region: ${{ env.AWS_REGION }}\n", 1)[1],
                "stop AWS region requirement",
            ),
            (
                workflow.replace(
                    "        uses: actions/setup-go@d35c59abb061a4a6fb18e82ac0862c26744d6ab5\n",
                    "",
                    1,
                ),
                "SDK Go setup action",
            ),
            (
                workflow.replace("          go-version-file: cyclops-cs/backend/go.mod\n", "", 1),
                "SDK Go version file",
            ),
            (
                workflow.replace("            --owners 099720109477 \\\n", "", 1),
                "EC2 Ubuntu AMI owner",
            ),
            (
                workflow.replace(
                    '            --filters "Name=name,Values=ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*" \\\n',
                    "",
                    1,
                ),
                "EC2 Ubuntu Noble AMI filter",
            ),
            (
                workflow.replace("            --query 'sort_by(Images, &CreationDate)[-1].ImageId' \\\n", "", 1),
                "EC2 latest AMI query",
            ),
            (
                workflow.replace('          test -n "$AMI_ID"\n', "", 1),
                "EC2 AMI nonempty validation",
            ),
            (
                workflow.replace('          test "$AMI_ID" != "None"\n', "", 1),
                "EC2 AMI None validation",
            ),
            (
                workflow.replace("  contents: read\n", "  id-token: write\n  contents: read\n", 1),
                "workflow-level EC2 OIDC permission",
            ),
            (
                workflow.replace("    if: ${{ github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]') }}\n", "", 1),
                "trusted EC2 start guard",
            ),
            (
                workflow.replace("    if: ${{ always() && (github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.head.repo.fork == false && github.actor != 'dependabot[bot]')) }}\n", "    if: ${{ always() }}\n", 1),
                "trusted unconditional EC2 cleanup",
            ),
            (
                workflow.replace("github_action_runner_label_prefix: cyclops-sdk-diag-${{ github.run_attempt }}-", "github_action_runner_label_prefix: cyclops-sdk-diag-"),
                "attempt-unique EC2 label",
            ),
            (
                workflow.replace("    if: ${{ always() && needs.uniffi-diagnostics-tests.result == 'success' && ((github.event_name == 'pull_request' && (github.event.pull_request.head.repo.full_name != github.repository || github.event.pull_request.head.repo.fork != false || github.actor == 'dependabot[bot]')) || needs.start-sdk-bindings-runner.result == 'success') }}\n", "", 1),
                "SDK coverage and diagnostic test guard",
            ),
            (
                workflow.replace("format('cyclops-sdk-diag-{0}-{1}', github.run_attempt, github.run_id) || 'ubuntu-24.04'", "'ubuntu-24.04'"),
                "SDK dynamic runner selection",
            ),
            (
                workflow.replace("tr '\\t ' '\\n'", "tr ' ' '\\n'", 1),
                "EC2 multi-ID splitting",
            ),
            (
                workflow.replace('"Key":"github_run_attempt","Value":"${{ github.run_attempt }}"', '"Key":"github_run_attempt","Value":"1"'),
                "EC2 run-attempt tag",
            ),
            (
                workflow.replace("      - name: Resolve EC2 runner instance\n", "      # - name: Resolve EC2 runner instance\n", 1),
                "EC2 resolve step",
            ),
            (
                workflow.replace('".github/scripts/uniffi-diagnostics/**"', '".github/scripts/uniffi-diagnostics-missing/**"', 1),
                "diagnostic dependency path trigger",
            ),
            (
                workflow.replace('    needs: uniffi-diagnostics-tests\n', '', 1),
                "EC2 start diagnostic test gate",
            ),
            (
                workflow.replace('    needs: [uniffi-diagnostics-tests, start-sdk-bindings-runner]\n', '    needs: start-sdk-bindings-runner\n', 1),
                "SDK diagnostic test gate",
            ),
            (
                workflow.replace('"Key":"github_repository_id","Value":"${{ github.repository_id }}"', '"Key":"github_repository_id","Value":"0"', 1),
                "EC2 repository ID tag",
            ),
            (
                workflow.replace('"Name=tag:github_repository_id,Values=${{ github.repository_id }}"', '"Name=tag:github_repository_id,Values=0"', 1),
                "EC2 resolve repository ID tag",
            ),
            (
                workflow.rsplit('"Name=tag:github_repository_id,Values=${{ github.repository_id }}"', 1)[0]
                + workflow.rsplit('"Name=tag:github_repository_id,Values=${{ github.repository_id }}"', 1)[1],
                "EC2 cleanup repository ID tag",
            ),
            (
                workflow.replace('      - name: Set up Node.js\n', "      - name: Set up Node.js\n        if: ${{ runner.environment == 'self-hosted' }}\n", 1),
                "unconditional SDK Node setup",
            ),
            (
                workflow.replace('      - name: Set up Node.js\n', '      - name: Missing Node.js setup\n', 1),
                "SDK Node setup step",
            ),
            (
                workflow.replace('actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020', 'actions/setup-node@0000000000000000000000000000000000000000', 1),
                "SDK Node setup action SHA",
            ),
            (
                workflow.replace('          node-version-file: cyclops-cs/.nvmrc\n', '          node-version-file: cyclops-cs/missing.nvmrc\n', 1),
                "SDK Node version file",
            ),
            (
                workflow.replace(
                    '      - name: Set up Node.js\n        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020\n        with:\n          node-version-file: cyclops-cs/.nvmrc\n',
                    '',
                    1,
                ).replace(
                    '      - name: Check generated bindings and generator regression harness\n        run: cyclops-cs/scripts/test-generate-sdk-bindings.sh\n',
                    '      - name: Check generated bindings and generator regression harness\n        run: cyclops-cs/scripts/test-generate-sdk-bindings.sh\n      - name: Set up Node.js\n        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020\n        with:\n          node-version-file: cyclops-cs/.nvmrc\n',
                    1,
                ),
                "SDK Node setup ordering",
            ),
            (
                workflow.replace('      - name: Install EC2 runner prerequisites\n', '      - name: Missing EC2 runner prerequisites\n', 1),
                "EC2 runner prerequisite step",
            ),
            (
                workflow.replace('          sudo apt-get install --yes unzip\n', '          sudo apt-get install --yes curl\n', 1),
                "EC2 runner unzip prerequisite",
            ),
            (
                workflow.replace('      - name: Install UniFFI diagnostic dependencies\n        if:', '      - name: Install UniFFI diagnostic dependencies\n        # if:', 1),
                "trusted diagnostic dependency route",
            ),
            (
                workflow.replace('      - name: Install compatibility binding generator\n        if:', '      - name: Install compatibility binding generator\n        # if:', 1),
                "untrusted hosted fallback route",
            ),
            (
                workflow.replace('MATCHING_INSTANCE_IDS="$(aws ec2 describe-instances', 'MATCHING_INSTANCE_IDS="" # aws ec2 describe-instances', 1),
                "EC2 cleanup fallback query",
            ),
            (
                workflow.replace('          if [ "$MATCHING_INSTANCE_IDS" = "None" ]; then\n            MATCHING_INSTANCE_IDS=""\n          fi\n', '', 1),
                "EC2 cleanup empty result normalization",
            ),
            (
                workflow.replace(
                    "  EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge\n",
                    "  UNRELATED: 'first\n    # EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge\n    final'\n",
                ),
                "multiline single-quoted EC2 workflow injection",
            ),
            (
                workflow.replace(
                    "  EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge\n",
                    '  UNRELATED: "first\n    # EC2_RUNNER_INSTANCE_TYPE: m7i.2xlarge\n    final"\n',
                ),
                "multiline double-quoted EC2 workflow injection",
            ),
        ]:
            require_workflow_rejected(fixture, mutated_workflow, description)
    print("SDK binding source checker comment-only self-test passed.")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true", help="prove comment-only evidence is ignored")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    try:
        if args.self_test:
            self_test()
        else:
            check(root)
    except (ContractFailure, tokenize.TokenError) as error:
        print(error, file=sys.stderr)
        return 1
    if not args.self_test:
        print("SDK binding source contract checks passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
