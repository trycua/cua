import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_gate_scope as scope  # noqa: E402
import ci_graph  # noqa: E402

CFG = scope.load_map()
GRAPH = ci_graph.load()
WORKFLOWS = sorted(scope.WORKFLOWS.glob('*.yml'))
MAPPED = {w for a in CFG['areas'].values() for w in a['workflows']}


def ignored(*files):
    return scope.scope(list(files), CFG, GRAPH)[1]


def areas_of(*files):
    return scope.scope(list(files), CFG, GRAPH)[0]


def test_relay_change_also_waits_on_the_spacesd_crates_that_link_the_relay():
    # cua-spacesd and cua-spacesd-server depend on cua-relay, so a relay change
    # waits on the spacesd area too (the hand-narrowed relay area alone missed it).
    areas, skip = scope.scope(['libs/cua-spacesd/crates/cua-relay/src/mesh.rs'], CFG, GRAPH)
    assert areas == ['relay', 'spacesd']
    assert not skip & {'ci-cua-relay-image.yml', 'ci-cua-spacesd.yml', 'e2e-cua-sdk.yml'}
    assert skip >= {'ci-cua-spaces-web.yml', 'ci-cua-spaces-macos.yml', 'ci-swift-lume.yml'}


def test_other_spacesd_change_still_waits_on_the_integration_jobs():
    skip = ignored('libs/cua-spacesd/crates/cua-spacesd/src/lib.rs')
    assert not skip & {'cd-image-linux.yml', 'e2e-cua-sdk.yml', 'ci-cua-sandbox-doctor.yml',
                       'ci-cua-oci-spacesd.yml', 'ci-cua-relay-image.yml'}


def test_a_change_in_two_areas_needs_both_areas_jobs():
    skip = ignored('libs/cua-spacesd/crates/cua-relay/src/mesh.rs', 'docs/content/a.mdx')
    assert 'ci-check-docs-links.yml' not in skip
    assert 'ci-cua-relay-image.yml' not in skip
    assert 'ci-cua-spaces-web.yml' in skip


@pytest.mark.parametrize('path', [
    'Cargo.lock', 'README.md', 'package.json', 'rust-toolchain.toml',
    '.github/workflows/ci-gate.yml', '.github/ci-areas.json', '.github/scripts/ci_graph.py',
    'scripts/ci/prepush.sh', 'nix/cua-driver/default.nix', 'libs/python/agent/uv.lock',
    'some/unmapped/dir/file.txt',
])
def test_shared_and_unknown_files_wait_on_everything(path):
    areas, skip = scope.scope(['libs/cua-spacesd/crates/cua-relay/a.rs', path], CFG, GRAPH)
    assert areas == ['all']
    assert skip == set()


def test_empty_change_list_waits_on_everything():
    assert scope.scope([], CFG, GRAPH) == (['all'], set())


# --- the package graph -----------------------------------------------------


def dependent_areas(pkg):
    return {scope.area_of(d + '/', CFG) for d in GRAPH.dependents({pkg})}


@pytest.mark.parametrize(('path', 'pkg'), [
    ('libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs', 'libs/cua-driver/rust/crates/cua-driver-core'),
    ('libs/cua-driver/rust/crates/cua-driver-contract/src/lib.rs', 'libs/cua-driver/rust/crates/cua-driver-contract'),
    ('libs/cua/crates/cua-proto/src/lib.rs', 'libs/cua/crates/cua-proto'),
    ('libs/cua/crates/cua-sandbox-core/src/lib.rs', 'libs/cua/crates/cua-sandbox-core'),
    ('libs/cua/proto/cua/v1/a.proto', 'libs/cua/crates/cua-proto'),
])
def test_a_shared_crate_waits_on_every_dependent_area_and_no_other(path, pkg):
    areas = set(areas_of(path))
    assert 'all' not in areas
    assert areas >= dependent_areas(pkg)
    assert areas == dependent_areas(pkg) | {scope.area_of(path, CFG)}


def test_driver_core_change_waits_on_its_spacesd_dependents_but_not_on_docs_only_jobs():
    skip = ignored('libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs')
    assert 'ci-cua-spacesd.yml' not in skip  # cua-spacesd depends on the driver core
    assert 'ci-rust-linux.yml' not in skip
    assert {'ci-cua-spaces-web.yml', 'ci-swift-lume.yml'} <= skip


@pytest.mark.parametrize(('path', 'workspace'), [
    ('libs/cua-driver/rust/Cargo.lock', 'libs/cua-driver/rust'),
    ('libs/cua-driver/rust/Cargo.toml', 'libs/cua-driver/rust'),
    ('libs/cua-driver/rust/.cargo/config.toml', 'libs/cua-driver/rust'),
    ('libs/cua-spacesd/Cargo.lock', 'libs/cua-spacesd'),
    ('libs/cua/Cargo.toml', 'libs/cua'),
    ('docs/pnpm-lock.yaml', 'docs'),
    ('libs/typescript/pnpm-workspace.yaml', 'libs/typescript'),
])
def test_a_workspace_file_affects_every_member_and_its_dependents(path, workspace):
    members = GRAPH.members(workspace)
    assert members
    expected = {scope.area_of(d + '/', CFG) for d in GRAPH.dependents(members)}
    assert set(areas_of(path)) == expected


def test_a_lockfile_in_one_workspace_does_not_wait_on_unrelated_lanes():
    assert areas_of('docs/pnpm-lock.yaml') == ['docs']
    skip = ignored('libs/cua-spacesd/Cargo.lock')
    assert {'ci-cua-spaces-web.yml', 'ci-swift-lume.yml', 'ci-check-docs.yml'} <= skip


def test_a_new_dependency_in_the_pull_request_counts(tmp_path):
    """The gate unions the base graph with the PR's manifests, so a PR that adds an edge waits on it."""
    def crate(root, rel, deps=''):
        (root / rel).mkdir(parents=True)
        (root / rel / 'Cargo.toml').write_text(f'[package]\nname = "{rel.rsplit("/", 1)[1]}"\n{deps}')
    base, head = tmp_path / 'base', tmp_path / 'head'
    crate(base, 'libs/cua/crates/a')
    crate(base, 'docs/b')
    crate(head, 'libs/cua/crates/a')
    crate(head, 'docs/b', '[dependencies]\na = { path = "../../libs/cua/crates/a" }\n')
    only_base = scope.load_graph([base])
    both = scope.load_graph([base, head])
    assert scope.scope(['libs/cua/crates/a/src/lib.rs'], CFG, only_base)[0] == ['cua']
    assert scope.scope(['libs/cua/crates/a/src/lib.rs'], CFG, both)[0] == ['cua', 'docs']


def test_a_dependent_package_outside_every_area_waits_on_everything(tmp_path):
    root = tmp_path / 'r'
    (root / 'libs/cua/crates/a').mkdir(parents=True)
    (root / 'libs/cua/crates/a/Cargo.toml').write_text('[package]\nname = "a"\n')
    (root / 'nowhere/c').mkdir(parents=True)
    (root / 'nowhere/c/Cargo.toml').write_text('[package]\nname = "c"\n[dependencies]\na = { path = "../../libs/cua/crates/a" }\n')
    assert scope.scope(['libs/cua/crates/a/x.rs'], CFG, scope.load_graph([root])) == (['all'], set())


def test_workspace_inherited_path_dependencies_are_edges(tmp_path):
    root = tmp_path / 'r'
    (root / 'ws/crates/a').mkdir(parents=True)
    (root / 'ws/crates/b').mkdir(parents=True)
    (root / 'ws/Cargo.toml').write_text(
        '[workspace]\nmembers = ["crates/*"]\n[workspace.dependencies]\na = { path = "crates/a" }\n')
    (root / 'ws/crates/a/Cargo.toml').write_text('[package]\nname = "a"\n')
    (root / 'ws/crates/b/Cargo.toml').write_text(
        '[package]\nname = "b"\n[target.\'cfg(unix)\'.dev-dependencies]\na = { workspace = true }\n')
    g = ci_graph.load(root)
    assert g.packages['ws/crates/b'].deps == {'ws/crates/a'}
    assert g.dependents({'ws/crates/a'}) == {'ws/crates/a', 'ws/crates/b'}
    assert g.workspace_file('ws/Cargo.lock') == 'ws'
    assert g.owner('ws/crates/b/src/lib.rs') == 'ws/crates/b'


@pytest.mark.skipif(shutil.which('cargo') is None, reason='needs cargo')
@pytest.mark.parametrize('workspace', ['libs/cua', 'libs/cua-driver/rust', 'libs/cua-spacesd'])
def test_cargo_edges_match_cargo_metadata(workspace):
    done = subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1', '--offline'],
                          cwd=scope.ROOT / workspace, capture_output=True, text=True)
    if done.returncode != 0:
        pytest.skip(f'cargo metadata failed: {done.stderr[-300:]}')
    for pkg in json.loads(done.stdout)['packages']:
        d = Path(pkg['manifest_path']).parent.relative_to(scope.ROOT).as_posix()
        want = {Path(dep['path']).relative_to(scope.ROOT).as_posix()
                for dep in pkg['dependencies'] if dep.get('path')} - {d}
        assert GRAPH.packages[d].deps == want, d


def test_unmapped_workflow_is_never_ignored():
    # A workflow absent from the map is required for every PR.
    for files in (['docs/content/a.mdx'], ['apps/cua-spaces-web/x.ts']):
        assert 'ci-check-emudhra-distrust.yml' not in ignored(*files)
        assert 'ci-check-emudhra-distrust.yml' not in MAPPED


@pytest.mark.parametrize(('glob', 'path', 'hit'), [
    ('libs/a/**', 'libs/a/b/c.rs', True),
    ('libs/a/**', 'libs/ab/c.rs', False),
    ('*', 'README.md', True),
    ('*', 'docs/README.md', False),
    ('**/Cargo.lock', 'Cargo.lock', True),
    ('**/Cargo.lock', 'a/b/Cargo.lock', True),
    ('libs/*/x.rs', 'libs/a/b/x.rs', False),
])
def test_glob_matching(glob, path, hit):
    assert bool(scope.glob_to_regex(glob).match(path)) is hit


def test_every_mapped_workflow_exists_and_every_area_name_is_listed_once():
    names = {w.name for w in WORKFLOWS}
    for area in CFG['areas'].values():
        assert set(area['workflows']) <= names
        assert len(area['workflows']) == len(set(area['workflows']))


@pytest.mark.parametrize('workflow', WORKFLOWS, ids=lambda w: w.name)
def test_map_covers_every_area_a_workflow_can_trigger_on(workflow):
    """The map may only narrow on purpose: a workflow whose own paths reach an area must
    be listed there, unless the area is marked `narrowed` (then its list is a subset)."""
    for name in scope.expected_areas(workflow, CFG):
        area = CFG['areas'][name]
        if area.get('narrowed'):
            continue
        assert workflow.name in area['workflows'], (
            f'{workflow.name} triggers on paths in area "{name}" but is not listed there; '
            f'add it to .github/ci-areas.json')


def test_the_gate_and_its_scope_files_are_shared():
    for f in ('.github/workflows/ci-gate.yml', '.github/ci-areas.json',
              '.github/scripts/ci_gate_scope.py', '.github/scripts/ci_graph.py'):
        assert scope.area_of(f, CFG) is None


@pytest.mark.parametrize(('body', 'ok'), [
    ('Fixes CUA-2338', True),
    ('Linear: https://linear.app/trycua/issue/CUA-2338/urgent-fix-lane', True),
    ('Incident #482: relay down', True),
    ('incident 2026-10-25 relay outage', True),
    ('urgent please', False),
    ('', False),
    (None, False),
    ('Co-authored by UTF-8 and sha-256', False),
    ('Linear: CLE-45', True),
])
def test_urgent_reference(body, ok):
    assert scope.urgent_reference_ok(body) is ok


def test_urgent_label_without_reference_fails_the_gate(tmp_path):
    body = tmp_path / 'body.txt'
    body.write_text('quick fix, no reference')
    done = subprocess.run(
        [sys.executable, str(scope.ROOT / '.github/scripts/ci_gate_scope.py'),
         '--labels', 'bug,urgent', '--body-file', str(body)],
        input='libs/cua-spacesd/crates/cua-relay/a.rs', capture_output=True, text=True)
    assert done.returncode == 1
    assert 'urgent' in done.stdout


def test_urgent_label_with_reference_uses_the_same_path_scope(tmp_path):
    body = tmp_path / 'body.txt'
    body.write_text('Fixes CUA-2338')
    out = tmp_path / 'out'
    done = subprocess.run(
        [sys.executable, str(scope.ROOT / '.github/scripts/ci_gate_scope.py'),
         '--labels', 'urgent', '--body-file', str(body)],
        input='libs/cua-spacesd/crates/cua-relay/a.rs', capture_output=True, text=True,
        env={**os.environ, 'GITHUB_OUTPUT': str(out)})
    assert done.returncode == 0, done.stdout
    assert 'areas=relay' in out.read_text()
    plain = subprocess.run(
        [sys.executable, str(scope.ROOT / '.github/scripts/ci_gate_scope.py')],
        input='libs/cua-spacesd/crates/cua-relay/a.rs', capture_output=True, text=True,
        env={**os.environ, 'GITHUB_OUTPUT': str(tmp_path / 'out2')})
    assert (tmp_path / 'out2').read_text() == out.read_text()


def test_gate_runs_the_scope_step_first_and_passes_its_output_to_the_wait_step():
    gate = yaml.safe_load((scope.WORKFLOWS / 'ci-gate.yml').read_text())
    steps = gate['jobs']['ci-gate']['steps']
    # The scope step runs right after the two selector checkouts; the hold-label step may sit before them.
    names = [st.get('name') for st in steps]
    assert names.index('Scope the gate to the changed areas') == names.index("Check out the pull request's manifests") + 1
    assert next(st for st in steps if st.get('name') == 'Scope the gate to the changed areas')['id'] == 'scope'
    wait = next(st for st in steps if st.get('name', '').startswith('Wait for every other'))
    assert wait['env']['IGNORE_RE'] == '${{ steps.scope.outputs.ignore_re }}'
    assert 'labeled' in gate[True]['pull_request']['types']
    assert 'edited' in gate[True]['pull_request']['types']
    assert 'merge_group' in gate[True]


def test_gate_run_filter_drops_only_ignored_workflows():
    script = yaml.safe_load((scope.WORKFLOWS / 'ci-gate.yml').read_text())['jobs']['ci-gate']['steps']
    script = next(st for st in script if st.get('name', '').startswith('Wait for every other'))['run']
    jq_filter = next(line.split("='", 1)[1].rsplit("'", 1)[0]
                     for line in script.splitlines() if line.strip().startswith('scope_jq='))
    runs = [{'path': '.github/workflows/cd-image-linux.yml@refs/pull/1/merge'},
            {'path': '.github/workflows/ci-cua-spacesd.yml'}]
    program = f'[.[] | select({jq_filter}) | .path] | length'
    def count(ignore):
        return int(subprocess.run(['jq', '-n', '--argjson', 'r', json.dumps(runs), f'$r | {program}'],
                                  capture_output=True, text=True,
                                  env={**os.environ, 'IGNORE_RE': ignore}).stdout)
    assert count('') == 2
    assert count('^(cd-image-linux\\.yml)$') == 1


def test_gate_runs_the_selector_from_the_base_commit_not_the_pull_request():
    """A PR that edits the map, the scripts or its manifests cannot narrow its own gate."""
    steps = yaml.safe_load((scope.WORKFLOWS / 'ci-gate.yml').read_text())['jobs']['ci-gate']['steps']
    by_name = {st.get('name'): st for st in steps}
    scope_step = by_name['Scope the gate to the changed areas']
    base = by_name['Check out the selector from the base commit']['with']
    head = by_name["Check out the pull request's manifests"]['with']
    run = scope_step['run']
    assert base['ref'] == '${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha }}'
    assert base['path'] == 'base'
    for f in ('/.github/ci-areas.json', '/.github/scripts/ci_gate_scope.py', '/.github/scripts/ci_graph.py'):
        assert f in base['sparse-checkout'].split()
        assert f not in head['sparse-checkout'].split()
    assert 'ref' not in head and head['path'] == 'head'
    assert 'python3 base/.github/scripts/ci_gate_scope.py' in run
    assert scope_step['env']['CI_GATE_HEAD_ROOT'].endswith('/head')


# --- narrowing and the execution scope (CUA-3856) ----------------------------------------------------

def test_every_narrowed_workflow_exists_and_names_real_packages():
    names = {w.name for w in WORKFLOWS}
    for workflow, rule in CFG['narrow'].items():
        assert workflow in names, workflow
        assert rule.get('packages') or rule.get('paths'), workflow
        for pkg in rule.get('packages', []):
            assert pkg in GRAPH.packages, f'{workflow}: {pkg} is not a package directory'


# A spaces app-core change (the Rust half of #6992, an 8 Nov copy pass): the Rust, web and app lanes
# that build the crate or its dependents run; the Python, Tauri-example and samples suites do not.
SPACES_CORE = ['libs/cua/crates/cua-spaces-app-core/src/first_run.rs']


@pytest.mark.parametrize('workflow', [
    'ci-test-python.yml', 'ci-openkoalabot-example-tauri.yml', 'ci-openkoalabot-example-samples.yml'])
def test_a_spaces_core_change_skips_suites_that_do_not_build_it(workflow):
    assert workflow in ignored(*SPACES_CORE)


@pytest.mark.parametrize('workflow', ['ci-cua-sdk.yml', 'ci-cua-spaces.yml', 'ci-cua-spaces-web.yml'])
def test_a_spaces_core_change_still_runs_the_rust_lanes_that_build_it(workflow):
    assert workflow not in ignored(*SPACES_CORE)


@pytest.mark.parametrize(('path', 'workflow'), [
    ('libs/cua/crates/cua-sdk/src/lib.rs', 'ci-test-python.yml'),
    ('libs/cua/crates/cua-proto/src/lib.rs', 'ci-test-python.yml'),  # in cua-sdk's closure
    ('libs/cua/proto/cua/v1/spaces.proto', 'ci-test-python.yml'),     # an input of cua-proto
    ('libs/cua/Cargo.lock', 'ci-test-python.yml'),                     # the workspace
    ('libs/python/core/core/x.py', 'ci-test-python.yml'),
    ('libs/cua/crates/cua-spacesd-client/src/lib.rs', 'ci-openkoalabot-example-tauri.yml'),
    ('examples/openkoalabot-example-tauri/src/App.tsx', 'ci-openkoalabot-example-tauri.yml'),
    ('libs/cua/typescript/src/index.ts', 'ci-openkoalabot-example-samples.yml'),
    ('libs/cua/swift/Sources/CuaSDK/CuaSDK.swift', 'ci-openkoalabot-example-samples.yml'),
    ('.github/workflows/ci-test-python.yml', 'ci-test-python.yml'),
])
def test_a_narrowed_workflow_still_runs_when_what_it_builds_changes(path, workflow):
    assert workflow not in ignored(path)


def test_a_web_only_change_runs_only_the_web_lane():
    web = ['apps/cua-spaces-web/src/components/shell/sidebar.tsx',
           'apps/cua-spaces-web/src/__tests__/tips.test.tsx']
    assert areas_of(*web) == ['spaces-web']
    skipped = ignored(*web)
    assert 'ci-cua-spaces-web.yml' not in skipped
    for heavy in ('ci-cua-sdk.yml', 'ci-rust-linux.yml', 'ci-swift-lume.yml', 'ci-test-python.yml',
                  'ci-openkoalabot-example-samples.yml', 'e2e-cua-sdk.yml'):
        assert heavy in skipped, heavy


def test_workflow_mode_writes_the_skip_output(tmp_path):
    out = tmp_path / 'out'
    def decide(workflow, files):
        out.write_text('')
        subprocess.run([sys.executable, str(scope.ROOT / '.github/scripts/ci_gate_scope.py'), '--workflow', workflow],
                       input='\n'.join(files), text=True, check=True, capture_output=True,
                       env={**os.environ, 'GITHUB_OUTPUT': str(out)})
        return dict(line.split('=', 1) for line in out.read_text().splitlines())['skip']
    assert decide('ci-test-python.yml', SPACES_CORE) == 'true'
    assert decide('ci-cua-sdk.yml', SPACES_CORE) == 'false'
    # A workflow the map does not know (CI: Quick itself, a new workflow) always runs.
    assert decide('ci-quick.yml', SPACES_CORE) == 'false'
    # No file list (a failed diff would be an error, never this) runs everything.
    assert decide('ci-test-python.yml', []) == 'false'
