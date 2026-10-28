import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_gate_scope as scope  # noqa: E402

CFG = scope.load_map()
WORKFLOWS = sorted(scope.WORKFLOWS.glob('*.yml'))
MAPPED = {w for a in CFG['areas'].values() for w in a['workflows']}


def ignored(*files):
    return scope.scope(list(files), CFG)[1]


def test_relay_only_change_waits_on_relay_and_rust_jobs_only():
    areas, skip = scope.scope(['libs/cua-spacesd/crates/cua-relay/src/mesh.rs'], CFG)
    assert areas == ['relay']
    assert not skip & {'ci-cua-relay-image.yml', 'ci-cua-spacesd.yml'}
    # The slow image, SDK and doctor jobs belong to other areas and do not gate a relay change.
    assert skip >= {'cd-image-linux.yml', 'e2e-cua-sdk.yml', 'ci-cua-sandbox-doctor.yml',
                    'ci-cua-oci-spacesd.yml', 'ci-cua-spaces-web.yml', 'ci-cua-spaces-macos.yml'}


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
    'Cargo.lock', 'libs/cua-driver/rust/Cargo.lock', 'README.md', 'package.json',
    '.github/workflows/ci-gate.yml', '.github/ci-areas.json', 'libs/cua/proto/a.proto',
    'libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs',
    'libs/cua/crates/cua-proto/src/lib.rs', 'rust-toolchain.toml',
    'some/unmapped/dir/file.txt',
])
def test_shared_and_unknown_files_wait_on_everything(path):
    areas, skip = scope.scope(['libs/cua-spacesd/crates/cua-relay/a.rs', path], CFG)
    assert areas == ['all']
    assert skip == set()


def test_empty_change_list_waits_on_everything():
    assert scope.scope([], CFG) == (['all'], set())


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
              '.github/scripts/ci_gate_scope.py'):
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
    assert steps[1]['id'] == 'scope'
    assert steps[2]['env']['IGNORE_RE'] == '${{ steps.scope.outputs.ignore_re }}'
    assert 'labeled' in gate[True]['pull_request']['types']
    assert 'edited' in gate[True]['pull_request']['types']
    assert 'merge_group' in gate[True]


def test_gate_run_filter_drops_only_ignored_workflows():
    script = yaml.safe_load((scope.WORKFLOWS / 'ci-gate.yml').read_text())['jobs']['ci-gate']['steps'][2]['run']
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
