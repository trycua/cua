#!/usr/bin/env python3
"""Validate executable Task 11 binding-contract fixture structure."""

from __future__ import annotations

import argparse
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


def executable_source(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".py":
        return strip_python_comments_and_strings(text)
    if path.suffix in {".kt", ".kts", ".swift"}:
        return strip_c_like_comments(text)
    if path.suffix == ".rb":
        return strip_ruby_comments(text)
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


def require_lifecycle(source: str, path: Path, label: str, queue_marker: str, calls: list[str], exhausted: str) -> None:
    checks = [(f"{label} request queue", queue_marker)]
    checks.extend((f"{label} lifecycle {call}", call) for call in calls)
    checks.append((f"{label} queue exhaustion", exhausted))
    require_all(source, path, checks)


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
        "kotlin_build": bindings / "kotlin/build.gradle.kts",
        "swift_runner": root / "cyclops-cs/scripts/run-swift-sdk-binding.sh",
        "ruby_runner": root / "cyclops-cs/scripts/run-ruby-sdk-binding.sh",
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

    require_all(source["kotlin_live"], files["kotlin_live"], [
        ("Kotlin live HTTP callback", r"class\s+JavaHttpClient\s*:\s*HttpClient"),
        ("Kotlin live typed pool", r"PoolSpec\(.*SandboxService\(\"mcp\",\s*3000u"),
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
        ("Swift live typed pool", r"PoolSpec\(.*SandboxService\(name:\s*\"mcp\",\s*targetPort:\s*3000"),
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
        ("Ruby live Net HTTP callback", r"class\s+NetHttpClient\s*<\s*CyclopsSdk::HttpClient"),
        ("Ruby live typed pool", r"CyclopsSdk::PoolSpec\.new.*CyclopsSdk::SandboxService\.new\(name:\s*\x27mcp\x27,\s*target_port:\s*3000"),
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
        ("Ruby callback transport constructor", r"raise\s+CyclopsSdk::HttpError::Transport\.new\("),
        ("Ruby future transport assertion", r"rescue\s+CyclopsSdk::SdkError::Transport\s*=>\s*error"),
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
    ])
    require(files["python_live"].read_text(encoding="utf-8"), "Python live environment", r"os\.environ\[\"CUA_CLIENT_ID\"\].*os\.environ\[\"CUA_IMAGE\"\]", files["python_live"])
    require_all(source["python_live"], files["python_live"], [
        ("Python live HTTP callback", r"class\s+UrlLibHttpClient\(HttpClient\).*async\s+def\s+execute"),
        ("Python live typed pool", r"PoolSpec\(.*SandboxService\(.*target_port=3000"),
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


def self_test() -> None:
    samples = {
        "kotlin": (strip_c_like_comments, "/* throw HttpException.Transport() */\n", r"HttpException\.Transport"),
        "swift": (strip_c_like_comments, "// actor ScriptedHttpClient: HttpClient\n", r"actor\s+ScriptedHttpClient"),
        "ruby": (strip_ruby_comments, "=begin\nThread.new do\n=end\n# Mutex.new\n", r"Thread\.new|Mutex\.new"),
        "python": (strip_python_comments_and_strings, '\"\"\" asyncio.gather(sdk.service_request()) \"\"\"\n# expected_service_calls()\n', r"asyncio\.gather|expected_service_calls"),
    }
    for language, (stripper, comment_only, pattern) in samples.items():
        if re.search(pattern, stripper(comment_only), flags=re.DOTALL):
            raise ContractFailure(f"comment-only negative self-test failed for {language}")

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
    print("SDK binding source checker comment-only self-test passed.")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true", help="prove comment-only evidence is ignored")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    try:
        check(root)
        if args.self_test:
            self_test()
    except (ContractFailure, tokenize.TokenError) as error:
        print(error, file=sys.stderr)
        return 1
    print("SDK binding source contract checks passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
