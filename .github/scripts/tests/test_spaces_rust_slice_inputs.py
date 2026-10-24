"""The compiled SwiftUI Rust slice is keyed by everything its build reads.

`ci-cua-spaces-macos.yml` restores main's compiled libcua_spaces_ffi when a pull
request's cache key equals the one main built. That is only safe if the key
changes whenever any file the compiler or a build script reads changes. These
tests pin that: the inputs are derived from the code (not listed by hand), any
change to one of them changes the key, the producer workflow triggers on each
of them, and the producer checks the claim against a real build.
"""

import hashlib
import importlib.util
import re
import subprocess
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / ".github/scripts/spaces_rust_slice_inputs.py"
spec = importlib.util.spec_from_file_location("spaces_rust_slice_inputs", SCRIPT)
inputs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inputs)

ACTION = yaml.safe_load((ROOT / ".github/actions/spaces-rust-slice/action.yml").read_text())
PRODUCER = yaml.safe_load((ROOT / ".github/workflows/ci-cua-spaces-rust-slice.yml").read_text())
MACOS = yaml.safe_load((ROOT / ".github/workflows/ci-cua-spaces-macos.yml").read_text())

needs_toml = pytest.mark.skipif(inputs._load_toml is None, reason="no TOML parser for the dependency closure")


def tracked(*specs):
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", *specs], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return [p for p in out.stdout.split("\0") if p]


def glob_matches(pattern, path):
    """GitHub's `paths:` globs: `**` crosses directories, `*` does not."""
    regex = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            regex += "(?:.*/)?"
            i += 3
        elif pattern.startswith("**", i):
            regex += ".*"
            i += 2
        elif pattern[i] == "*":
            regex += "[^/]*"
            i += 1
        else:
            regex += re.escape(pattern[i])
            i += 1
    return re.fullmatch(regex, path) is not None


@pytest.fixture(scope="module")
def extra():
    return inputs.extra_inputs(ROOT)


@pytest.fixture(scope="module")
def lines():
    return inputs.listing(ROOT)


def key_of(lines):
    return inputs.hash_listing(lines)


def with_changed_blob(lines, path):
    out = []
    hit = False
    for line in lines:
        meta, name = line.split("\t", 1)
        if name == path:
            mode, oid, stage = meta.split()
            oid = hashlib.sha1(oid.encode()).hexdigest()  # another blob
            line = f"{mode} {oid} {stage}\t{name}"
            hit = True
        out.append(line)
    assert hit, f"{path} is not in the key listing"
    return out


# ---------------------------------------------------------------- the inputs


@needs_toml
def test_the_closure_is_what_the_two_packages_build():
    crates = inputs.closure(ROOT)
    for name in (
        "cua-spaces-ffi",
        "cua-spaces-cli",
        "cua-spaces-app-core",
        "cua-sdk",
        "cua-image",
        "cua-host",
        "cua-agent-setup",
        "cua-telemetry",
    ):
        assert name in crates, name
    # A workspace crate nothing in the slice depends on is not scanned.
    assert "cua-bindgen" in inputs._workspace_members(ROOT)
    assert "cua-bindgen" not in crates


@needs_toml
def test_the_files_outside_the_hashed_tree_are_derived_from_the_code(extra):
    # palette.rs embeds it with include_str!; the reviewer's finding.
    assert "apps/cua-spaces-web/src/lib/keybindings.json" in extra
    # wizard.rs and cua-cli include it; cua-image and cua-telemetry's build scripts copy it.
    assert "libs/images/sandbox-images.json" in extra
    # cua-host's build script.
    assert "libs/cua-spacesd/VERSION" in extra
    # cua-agent-setup's build script embeds the skills; the hash drops libs/cua Markdown.
    assert "libs/cua/skills" in extra
    # Cargo merges the repository's .cargo/config.toml into every build below it.
    assert ".cargo" in extra
    # Read only by unit tests (`#[cfg(test)]`, `#[test]`) or tests/: not in the library.
    for test_only in (
        "docs/content/docs/cua-sdk/guides/your-cloud-safety.mdx",
        "scripts/images/check-tag-safety.sh",
        "scripts/images/manifests/macos-26.json",
        "libs/images/runtime-gvisor/versions.env",
        "tests/compat/e2b/fixtures/signature-vectors.json",
    ):
        assert test_only not in extra
    for path in extra:
        assert tracked(path), f"{path} is not tracked"


@needs_toml
def test_nothing_inside_libs_cua_is_left_out_unless_the_build_reads_it(extra):
    # The skills (embedded by cua-agent-setup) and the proto directory (the
    # contract the build compiles) hold Markdown the hash would otherwise drop.
    inside = [p for p in extra if p.startswith("libs/cua/")]
    assert inside == ["libs/cua/proto", "libs/cua/skills"]


# ------------------------------------------------------------------ the key


@needs_toml
def test_changing_any_input_changes_the_key(lines, extra):
    base = key_of(lines)
    paths = {line.split("\t", 1)[1] for line in lines}
    checked = 0
    for entry in extra:
        for path in tracked(entry):
            assert path in paths, f"{path} (an input) is missing from the key's listing"
            assert key_of(with_changed_blob(lines, path)) != base, path
            checked += 1
    assert checked >= 10
    # And a file that is in the hashed tree.
    first = next(p for p in sorted(paths) if p.startswith("libs/cua/crates/cua-sdk/src/"))
    assert key_of(with_changed_blob(lines, first)) != base


@needs_toml
def test_the_pull_request_that_only_edits_one_of_them_misses(lines):
    """The reviewer's case: keybindings.json or sandbox-images.json alone."""
    for path in ("apps/cua-spaces-web/src/lib/keybindings.json", "libs/images/sandbox-images.json"):
        assert key_of(with_changed_blob(lines, path)) != key_of(lines)


@needs_toml
def test_what_the_build_does_not_read_does_not_change_the_key(lines):
    paths = {line.split("\t", 1)[1] for line in lines}
    # A README or changelog edit must not cost a rebuild (`:(exclude,glob)**/*.md`
    # in the old key did not exclude them).
    assert "libs/cua/README.md" not in paths and "libs/cua/CHANGELOG.md" not in paths
    assert not any(p.startswith("libs/cua/crates/") and p.endswith(".md") for p in paths)
    assert not any(p.startswith(("libs/cua/python/", "libs/cua/typescript/", "libs/cua/kotlin/")) for p in paths)
    # The skills' Markdown is read (embedded), so it is hashed.
    assert any(p.startswith("libs/cua/skills/") and p.endswith(".md") for p in paths)
    # Order and duplicates do not matter.
    assert key_of(list(reversed(lines)) + lines[:3]) == key_of(lines)


def test_the_action_keys_the_cache_on_the_script():
    steps = {s.get("id"): s for s in ACTION["runs"]["steps"] if s.get("id")}
    run = steps["key"]["run"]
    assert "python3 .github/scripts/spaces_rust_slice_inputs.py key" in run
    assert "git ls-files" not in run  # no second, hand-kept copy of the input list
    assert re.search(r"swiftui-rust-v3-.*\$\{inputs_hash\}", run)


# --------------------------------------------------------------- the triggers


@needs_toml
def test_the_producer_runs_on_main_when_any_input_changes(extra):
    patterns = PRODUCER[True]["push"]["paths"]
    for entry in extra:
        for path in tracked(entry):
            assert any(glob_matches(p, path) for p in patterns), f"{path} does not trigger the producer"
    for path in ("libs/cua/crates/cua-sdk/src/lib.rs", ".github/scripts/spaces_rust_slice_inputs.py"):
        assert any(glob_matches(p, path) for p in patterns), path


def test_the_producer_and_the_consumer_run_when_the_key_logic_changes():
    script = ".github/scripts/spaces_rust_slice_inputs.py"
    assert script in PRODUCER[True]["pull_request"]["paths"]
    assert script in MACOS[True]["pull_request"]["paths"]
    assert script in MACOS[True]["push"]["paths"]


def test_the_macos_checks_still_run_for_the_two_files_that_started_this():
    for event in ("pull_request", "push"):
        paths = MACOS[True][event]["paths"]
        assert "apps/cua-spaces-web/src/lib/keybindings.json" in paths
        assert "libs/images/sandbox-images.json" in paths


# ---------------------------------------------------------- the ground truth


def test_the_producer_verifies_the_key_against_its_build_and_the_release_does_not():
    producer_step = next(
        s for s in PRODUCER["jobs"]["slice"]["steps"] if s.get("uses") == "./.github/actions/spaces-rust-slice"
    )
    assert producer_step["with"]["verify-key"] == "true"
    assert ACTION["inputs"]["verify-key"]["default"] == "false"
    for workflow in ("cd-cua-spaces.yml", "cd-cua-spaces-build.yml", "ci-cua-spaces-macos.yml"):
        assert "verify-key" not in (ROOT / ".github/workflows" / workflow).read_text()
    verify = next(s for s in ACTION["runs"]["steps"] if s.get("name", "").startswith("The cache key covers"))
    assert "inputs.verify-key == 'true'" in verify["if"] and "inputs.build == 'true'" in verify["if"]
    assert "spaces_rust_slice_inputs.py verify" in verify["run"]


@needs_toml
def test_verify_reports_a_file_the_build_read_and_the_key_misses(tmp_path):
    deps = tmp_path / "deps"
    build = tmp_path / "build" / "cua-image-0123456789abcdef"
    deps.mkdir()
    build.mkdir(parents=True)
    hashed = "crates/cua-sdk/src/lib.rs"
    missed = "../../docs/content/docs/cua-sdk/guides/your-cloud-safety.mdx"
    (deps / "cua_sdk-1.d").write_text(
        f"{tmp_path}/deps/cua_sdk-1.rmeta: {hashed} {missed}\n\n{hashed}:\n{missed}:\n"
    )
    (build / "output").write_text(
        f"cargo:rerun-if-changed={ROOT}/libs/cua/crates/cua-image/../../../images/sandbox-images.json\n"
        "cargo:rerun-if-changed=../../.git/HEAD\n"
    )
    assert inputs.verify(ROOT, tmp_path) == ["docs/content/docs/cua-sdk/guides/your-cloud-safety.mdx"]


@needs_toml
def test_verify_is_quiet_when_the_key_covers_everything(tmp_path):
    (tmp_path / "deps").mkdir()
    (tmp_path / "deps" / "app_core-1.d").write_text(
        "out.rmeta: crates/cua-spaces-app-core/src/palette.rs "
        "../../apps/cua-spaces-web/src/lib/keybindings.json "
        "../../libs/images/sandbox-images.json /Users/runner/.cargo/registry/src/x/serde.rs\n"
    )
    assert inputs.verify(ROOT, tmp_path) == []


@needs_toml
def test_verify_expands_a_directory_the_build_script_watches(tmp_path):
    build = tmp_path / "build" / "cua-agent-setup-0123456789abcdef"
    build.mkdir(parents=True)
    (tmp_path / "deps").mkdir()
    # The skills (hashed, Markdown included) and a directory the key does not cover.
    (build / "output").write_text(
        f"cargo:rerun-if-changed={ROOT}/libs/cua/crates/cua-agent-setup/../../skills\n"
        f"cargo:rerun-if-changed={ROOT}/scripts/images/manifests\n"
    )
    missing = inputs.verify(ROOT, tmp_path)
    assert missing and all(m.startswith("scripts/images/manifests/") for m in missing)


def test_dep_info_with_escaped_spaces_and_empty_rules():
    text = "target/x.rmeta: a/b.rs c\\ d.rs \\\n  e.rs\n\na/b.rs:\nc\\ d.rs:\n"
    assert inputs._depinfo_paths(text) == {"a/b.rs", "c d.rs", "e.rs"}


# -------------------------------------------------------------- the scanner


def reads_of(tmp_path, files, name="crate"):
    crate = tmp_path / name
    (crate / "src").mkdir(parents=True, exist_ok=True)
    for name, text in files.items():
        (crate / name).write_text(text)
    return {str(p.relative_to(tmp_path)) for p in inputs.source_reads(crate)}


def test_a_read_in_ordinary_code_counts_and_one_in_a_test_module_does_not(tmp_path):
    src = (
        'const A: &str = include_str!("../../a.json");\n'
        "#[cfg(test)]\nmod tests {\n"
        '    const B: &str = include_str!("../../b.json");\n'
        '    fn f() { let s = "}"; let c = \'}\'; let _ = include_str!("../../c.json"); }\n'
        "}\n"
        'const D: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/d.bin"));\n'
        '#[test]\nfn t() { include_str!("../../e.json"); }\n'
        'const OUT: &str = include_str!(concat!(env!("OUT_DIR"), "/generated.rs"));\n'
    )
    assert reads_of(tmp_path, {"src/lib.rs": src}) == {"a.json", "crate/d.bin"}


def test_text_that_only_looks_like_code_is_not_a_read_or_a_test_boundary(tmp_path):
    src = (
        '// include_str!("../../comment.json")\n'
        '/* #[cfg(test)] */\n'
        'const R: &str = r#"include_str!("../../raw.json") #[cfg(test)] {"#;\n'
        'fn after() { let _ = include_str!("../../after.json"); }\n'
    )
    assert reads_of(tmp_path, {"src/lib.rs": src}) == {"after.json"}


def test_a_cfg_test_item_without_a_body_and_an_inner_attribute(tmp_path):
    assert reads_of(
        tmp_path,
        {"src/lib.rs": '#[cfg(test)]\nmod tests;\nconst A: &str = include_str!("../../a.json");\n'},
    ) == {"a.json"}
    assert (
        reads_of(
            tmp_path,
            {"src/lib.rs": '#![cfg(test)]\nconst A: &str = include_str!("../../a.json");\n'},
            name="inner",
        )
        == set()
    )


def test_cfg_not_test_and_cfg_any_test_are_code_that_ships(tmp_path):
    src = (
        '#[cfg(not(test))]\nconst A: &str = include_str!("../../a.json");\n'
        '#[cfg(any(test, feature = "x"))]\nconst B: &str = include_str!("../../b.json");\n'
    )
    assert reads_of(tmp_path, {"src/lib.rs": src}) == {"a.json", "b.json"}


def test_a_build_script_reads_every_dot_dot_literal_and_path_attributes(tmp_path):
    build = (
        'const FILES: &[(&str, &str)] = &[("../x.json", "x.json"), ("../skills", "skills")];\n'
        'fn main() { let _ = "not/a/parent"; let _ = "../.git/HEAD"; }\n'
    )
    lib = '#[path = "../shared/mod.rs"]\nmod shared;\n'
    assert reads_of(tmp_path, {"build.rs": build, "src/lib.rs": lib}) == {
        "x.json",
        "skills",
        ".git/HEAD",
        "crate/shared/mod.rs",
    }
