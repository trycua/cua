"""Shared-policy native proof cells, using normal direct Driver MCP.

Run one cell per prepared disposable Hyprland desktop/document. The controller
must open a fresh document from production_app_smoke.create_documents and supply
its exact target, then restore a distinct foreground client before starting.
Every action refuses if its target is the primary client. This runner never
launches apps, installs policy, signs grants,
or sends plugin input. It uses the existing process-startup managed-policy path;
an inherited managed/user policy is a blocker, never silently replaced.

Plan: {"case": "no_manifest_allow|resource_allow|resource_wrong_pid|resource_wrong_window|
managed_allow|managed_deny", "mode": "unrestricted", "app": "calc|inkscape",
"target": {"pid": 20, "window_id": 200}, "document": "/disposable/document",
"disposable": true}. Mode defaults to unrestricted (the direct-MCP equivalent
of --dangerously-bypass-approvals); standard/bounded are explicit policy cells.
For each denial the same initial action is attempted in a fresh permitted
runtime, followed by the remaining fixed app smoke steps and saved-file oracle.
All steps have Driver before/after snapshots. Denial observations and window
discovery use a separate unrestricted observer: exact window scope cannot
observe excluded windows or enumerate the whole application.

CLI requires --plan, --evidence, --driver, --plugin, --source, --source-sha,
--trace-socket. Import verify_evidence to recheck raw records and saved documents.
Run the five manifest cells for each app/profile and no_manifest_allow for
standard/unrestricted to build a matrix; each needs a fresh
synthetic document. These cells prove shared policy at the native dispatch
boundary, not cached authorization, concurrency, hostile same-user isolation,
managed deployment/immutability, or the complete desktop certification matrix.
"""
import argparse
from production_app_smoke import add_provenance_arguments
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import xml.etree.ElementTree as ET
import zipfile

from driver_input_live import wait_for, wm
from primary_trace import Trace, analyze
from production_app_smoke import NS, ground, provenance, rectangle, verify_calc, verify_inkscape
from production_mcp import DirectMCP
from production_realapp_proof import (
    SMOKE_STEPS, app_process_identity, assert_no_dispatch, capacity_lane,
    check_response, document_root, trace_interval,
)


CASES = ('no_manifest_allow', 'resource_allow', 'resource_wrong_pid', 'resource_wrong_window',
         'managed_allow', 'managed_deny')
DENIALS = frozenset(('resource_wrong_pid', 'resource_wrong_window', 'managed_deny'))
TOOLS = ['get_window_state', 'press_key', 'hotkey']
MANAGED_ENV = 'CUA_DRIVER_MANAGED_POLICY_FILE'
POLICY_ENV = (MANAGED_ENV, 'CUA_DRIVER_POLICY_FILE')
# Bound Inkscape observations except before select and move: their grounding
# needs trailing menu nodes and selection status that the cap can omit. Keep
# default depth and images, and fail closed if either grounding check is unavailable.
POLICY_SNAPSHOT_LIMITS = {'inkscape': {'max_elements': 2500}}


def validate_plan(plan):
    assert set(plan) <= {'case', 'mode', 'app', 'target', 'document', 'disposable'}
    assert plan.get('disposable') is True, 'requires a prepared disposable desktop'
    assert plan['case'] in CASES and plan['app'] in SMOKE_STEPS
    assert plan.get('mode', 'unrestricted') in ('standard', 'bounded', 'unrestricted')
    assert plan['case'] != 'no_manifest_allow' or plan.get('mode', 'unrestricted') != 'bounded', \
        'bounded cannot admit an action without a manifest'
    assert set(plan['target']) == {'pid', 'window_id'}
    assert type(plan['target']['pid']) is int and 0 < plan['target']['pid'] <= 2**31 - 1
    assert type(plan['target']['window_id']) is int and 0 < plan['target']['window_id'] <= 2**64 - 1
    assert isinstance(plan['document'], str) and Path(plan['document']).is_absolute()
    expected_name = 'cua-smoke-calc.ods' if plan['app'] == 'calc' else 'cua-smoke-inkscape.svg'
    assert Path(plan['document']).name == expected_name, 'requires the canonical synthetic document'


def documents_for_case(plan, *, control=False):
    """JSON is valid YAML; only exact, synthetic typed window resources are used."""
    validate_plan(plan)
    if plan['case'] == 'no_manifest_allow':
        return None, None
    target = dict(plan['target'])
    if not control and plan['case'] == 'resource_wrong_pid':
        target['pid'] = target['pid'] + 1 if target['pid'] < 2**31 - 1 else 1
    if not control and plan['case'] == 'resource_wrong_window':
        target['window_id'] = target['window_id'] + 1 if target['window_id'] < 2**64 - 1 else 1
    manifest = {'version': 3, 'expires_after': '5m', 'idle_timeout': '2m',
                'allow': {'tools': list(TOOLS)}, 'resources': {'desktop': {'windows': [target]}}}
    managed = None
    if plan['case'].startswith('managed_'):
        managed = {'allow': {'tools': list(TOOLS)}}
        if not control and plan['case'] == 'managed_deny':
            tool = next(iter(SMOKE_STEPS[plan['app']].values()))[0]
            managed['deny'] = {'tools': [tool]}
    return manifest, managed


def expected_refusal(plan):
    assert plan['case'] in DENIALS
    if plan['case'] == 'managed_deny':
        tool = next(iter(SMOKE_STEPS[plan['app']].values()))[0]
        return {'code': 'permission_denied',
                'message': f"Permission denied: managed policy: tool '{tool}' is explicitly denied"}
    target = plan['target']
    return {'code': 'bounded_resource_outside_manifest',
            'message': 'protected resource is outside the capability manifest: '
                       f"desktop pid {target['pid']} window {target['window_id']} "
                       'is outside the capability manifest'}


def check_refusal(result, expected):
    """Accept exact common core/MCP admission envelopes, never plugin errors."""
    assert result.get('isError') is True, 'denial must be an MCP error'
    content = result.get('structuredContent')
    text = [{'type': 'text', 'text': expected['message']}]
    if expected['code'] == 'permission_denied' and content == {'code': 'permission_denied'}:
        assert result.get('content') == text, 'wrong MCP permission refusal'
        return 'mcp-tool-admission'
    assert content == {'status': 'refused', 'refusal': expected}, 'wrong common policy refusal'
    assert result.get('content', text) == text, 'conflicting refusal text'
    return 'core-resource-admission' if expected['code'] != 'permission_denied' else 'core-admission'


def start_client(driver, directory, mode, manifest=None, managed=None):
    """Serial CLI-only setup: inject only during child spawn and restore on error.

    Do not call this helper from a multi-threaded embedding. Run this module as
    a child process instead. DirectMCP snapshots its environment in __init__.
    No installed file or pre-existing ceiling is changed, even transiently.
    """
    assert not any(name in os.environ for name in POLICY_ENV), \
        'inherited managed/user policy: use a clean disposable process environment'
    profile = {'mode': mode, 'acknowledge_unrestricted': mode == 'unrestricted'}
    if manifest is not None:
        path = directory / 'manifest.yaml'
        with path.open('x') as stream:
            json.dump(manifest, stream, indent=2)
        profile.update(manifest=str(path), approve_manifest=True)
    if managed is not None:
        path = directory / 'managed.yaml'
        with path.open('x') as stream:
            json.dump(managed, stream, indent=2)
        os.environ[MANAGED_ENV] = str(path.resolve(strict=True))
    try:
        return DirectMCP(driver, directory, profile)
    finally:
        if managed is not None:
            os.environ.pop(MANAGED_ENV, None)


def check_snapshot(snapshot, plan):
    assert not snapshot.get('isError'), 'Driver observation failed'
    content = snapshot['structuredContent']
    assert all(type(content.get(key)) is int and content[key] == value
               for key, value in plan['target'].items()), 'snapshot target identity differs'
    assert content.get('screenshot_width', 0) > 0 and content.get('screenshot_height', 0) > 0
    assert Path(plan['document']).name in content.get('window_title', ''), 'wrong synthetic document'
    assert isinstance(content.get('window_bounds'), dict), 'missing target geometry'
    assert content.get('snapshot_id'), 'missing fresh snapshot identity'
    return content


def check_primary(primary, target):
    assert type(primary.get('pid')) is int and primary['pid'] > 0, 'missing primary client'
    assert isinstance(primary.get('address'), str) and int(primary['address'], 16) > 0
    assert primary['pid'] != target['pid'] and int(primary['address'], 16) != target['window_id'], \
        'target is the primary client; restore the separate foreground fixture first'


def snapshot(client, plan, observer=None, *, stage=None, before=False):
    # list_windows(pid) attests an application resource, which is deliberately
    # outside a window-only manifest. Keep discovery on the explicit observer.
    windows = (observer or client).tool('list_windows', {'pid': plan['target']['pid']})
    assert not windows.get('isError'), 'cannot resolve exact target'
    matches = [row for row in windows['structuredContent']['windows']
               if row.get('pid') == plan['target']['pid']]
    assert len(matches) == 1 and matches[0].get('window_id') == plan['target']['window_id'], \
        'target is stale or ambiguous'
    limits = ({} if before and stage in ('select', 'move') else
              POLICY_SNAPSHOT_LIMITS.get(plan['app'], {}))
    result = client.tool('get_window_state', {**plan['target'], 'session': 'policy-proof',
                         **limits})
    check_snapshot(result, plan)
    return result


def validate_initial_document(app, before):
    if app == 'calc':
        root = document_root(before, {'zip_member': 'content.xml'})
        initial = root.find('.//table:table-row/table:table-cell', NS)
        assert initial is not None and not ''.join(initial.itertext()).strip(), 'initial A1 is not blank'
        assert initial.get(f"{{{NS['office']}}}value-type") is None, 'initial A1 contains a value'
        assert initial.get(f"{{{NS['table']}}}formula") is None, 'initial A1 contains a formula'
    else:
        node, point = rectangle(before)
        assert point == (40, 60) and float(node.get('width')) == 80 and float(node.get('height')) == 50, \
            'initial rectangle differs from the synthetic fixture'


def document_effect(app, before, after):
    validate_initial_document(app, before)
    return (verify_calc if app == 'calc' else verify_inkscape)(before, after)


def saved_document(document, app, before):
    """Wait only for the existing save to finish; never repeat input."""
    def read_saved():
        value = document.read_bytes()
        try:
            document_effect(app, before, value)
        except (AssertionError, ET.ParseError, zipfile.BadZipFile):
            return None
        return value
    return wait_for(read_saved, timeout=5)


def verify_evidence(plan, evidence):
    """Recompute from raw results, full trace prefixes, and document bytes.

    This rejects contradictory/tampered artifacts, not a malicious same-user
    author who can rewrite all evidence coherently. No self-reported pass flag
    or app_effect_verified field is accepted as proof.
    """
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    validate_plan(plan)
    complete = evidence['trace']
    assert analyze(complete).get('telemetry_complete') is True, 'incomplete final trace'
    rows = evidence['actions']
    stages = list(SMOKE_STEPS[plan['app']])
    expected_stages = ([stages[0]] if plan['case'] in DENIALS else []) + stages
    assert len(rows) == len(expected_stages), 'missing or extra action'
    previous_time, previous_count = 0, 0
    control_pids = set()
    for index, (row, stage) in enumerate(zip(rows, expected_stages)):
        denied = plan['case'] in DENIALS and index == 0
        tool, args = SMOKE_STEPS[plan['app']][stage]
        assert row['stage'] == stage and row['tool'] == tool
        assert row['arguments'] == {**args, **plan['target'], 'session': 'policy-proof',
                                    'delivery_mode': 'background'}, 'action ownership changed'
        assert type(row['runtime_pid']) is int and row['runtime_pid'] > 0
        assert row['phase'] == ('deny' if denied else 'control')
        assert row['mode'] == plan.get('mode', 'unrestricted'), 'wrong runtime profile'
        check_primary(row['primary_before'], plan['target'])
        assert row['configuration'] == list(documents_for_case(plan, control=not denied)), \
            'wrong reviewed policy configuration'
        times = row['times_ns']
        assert len(times) == 4 and all(type(t) is int for t in times)
        assert previous_time < times[0] <= times[1] <= times[2] <= times[3], 'unordered observations'
        previous_time = times[3]
        before, after = (check_snapshot(row[key], plan) for key in ('before', 'after'))
        assert before['snapshot_id'] != after['snapshot_id'], 'after observation reused before state'
        assert before['window_bounds'] == after['window_bounds'], 'target geometry changed'
        ground(before, plan['app'], stage)
        for key in ('trace_before', 'trace_after'):
            page = row[key]
            trace_interval(page, page)
            assert page['count'] >= previous_count, 'trace intervals reordered'
            assert complete['events'][:page['count']] == page['events'], 'trace prefix changed'
            previous_count = page['count']
        if denied:
            check_refusal(row['response'], expected_refusal(plan))
            assert_no_dispatch(row['trace_before'], row['trace_after'])
        else:
            control_pids.add(row['runtime_pid'])
            check_response(row['response'], {'kind': 'dispatched'})
            capacity_lane(row['trace_before'], row['trace_after'], tool)
    assert len(control_pids) == 1, 'positive control changed runtime'
    if plan['case'] in DENIALS:
        assert rows[0]['runtime_pid'] not in control_pids, 'policy changed inside cached runtime'
        assert_no_dispatch(rows[0]['trace_before'], rows[1]['trace_before'])
    before = base64.b64decode(evidence['document_before'], validate=True)
    after = base64.b64decode(evidence['document_after'], validate=True)
    effect = document_effect(plan['app'], before, after)
    return {'result': 'passed', 'case': plan['case'], 'mode': plan.get('mode', 'unrestricted'),
            'app': plan['app'], 'positive_control': effect,
            'denial': expected_refusal(plan) if plan['case'] in DENIALS else None,
            'same_user_security_boundary': False, 'full_desktop_matrix': False}


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    plan = json.loads(args.plan.read_text())
    validate_plan(plan)
    assert not any(name in os.environ for name in POLICY_ENV), 'inherited managed/user policy'
    args.evidence.mkdir(parents=True, exist_ok=False)
    report = {'result': 'failed', 'plan': plan, 'actions': [], 'full_desktop_matrix': False}
    clients, trace, started = [], None, False
    def save():
        (args.evidence / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    def client(name, manifest=None, managed=None, mode=None):
        directory = args.evidence / name
        directory.mkdir()
        value = start_client(args.driver, directory, mode or plan.get('mode', 'unrestricted'),
                             manifest, managed)
        clients.append(value)
        return value
    try:
        report['provenance'] = (provenance(args, app_profile=plan['app_profile'])
                                if 'app_profile' in plan else provenance(args))
        report['provenance']['policy_runner_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        report['provenance']['app_process'] = app_process_identity(plan['app'], plan['target']['pid'])
        windows = json.loads(subprocess.check_output(['hyprctl', '-j', 'clients'], text=True, timeout=10))
        matches = [row for row in windows if row.get('pid') == plan['target']['pid']]
        assert len(matches) == 1 and matches[0].get('xwayland') is False, 'requires a native app window'
        assert int(matches[0]['address'], 16) == plan['target']['window_id'], 'native window identity differs'
        report['provenance']['native_window'] = matches[0]
        document = Path(plan['document']).resolve(strict=True)
        document_before = document.read_bytes()
        validate_initial_document(plan['app'], document_before)
        report['document_before'] = base64.b64encode(document_before).decode('ascii')
        observer = client('observer', mode='unrestricted')
        trace = Trace(args.trace_socket)
        assert trace.collect().get('active') is False, 'another proof owns the trace'
        trace.exchange('TRACE_START')
        started = True

        def action(actor, phase, stage, configuration):
            tool, arguments = SMOKE_STEPS[plan['app']][stage]
            observer_for_step = observer if phase == 'deny' else actor
            row = {'phase': phase, 'stage': stage, 'tool': tool, 'configuration': list(configuration),
                   'mode': plan.get('mode', 'unrestricted'),
                   'runtime_pid': actor.process.pid, 'arguments': {**arguments, **plan['target'],
                       'session': 'policy-proof', 'delivery_mode': 'background'}}
            report['actions'].append(row)
            row['before'] = snapshot(observer_for_step, plan, observer, stage=stage, before=True)
            ground(row['before']['structuredContent'], plan['app'], stage)
            row['primary_before'] = wm()
            check_primary(row['primary_before'], plan['target'])
            row['times_ns'] = [time.monotonic_ns()]
            row['trace_before'] = trace.collect()
            row['times_ns'].append(time.monotonic_ns())
            try:
                row['response'] = actor.tool(tool, row['arguments'])
            finally:
                row['times_ns'].append(time.monotonic_ns())
                # Independent observer survives an actor's poisoned MCP connection.
                row['after'] = snapshot(observer if actor.failed else observer_for_step,
                                        plan, observer, stage=stage)
                row['times_ns'].append(time.monotonic_ns())
                row['trace_after'] = trace.collect()
                save()
            if phase == 'deny':
                check_refusal(row['response'], expected_refusal(plan))
                assert_no_dispatch(row['trace_before'], row['trace_after'])
            else:
                check_response(row['response'], {'kind': 'dispatched'})
                capacity_lane(row['trace_before'], row['trace_after'], tool)

        if plan['case'] in DENIALS:
            config = documents_for_case(plan)
            denied = client('denied', *config)
            action(denied, 'deny', next(iter(SMOKE_STEPS[plan['app']])), config)
            denied.close()
        config = documents_for_case(plan, control=True)
        permitted = client('control', *config)
        for stage in SMOKE_STEPS[plan['app']]:
            action(permitted, 'control', stage, config)
        report['document_after'] = base64.b64encode(
            saved_document(document, plan['app'], document_before)).decode('ascii')
    except Exception as error:
        report['error'] = {'type': type(error).__name__, 'message': str(error)}
    finally:
        for value in reversed(clients):
            try:
                value.close()
            except Exception as error:
                report.setdefault('cleanup_errors', []).append(str(error))
        if trace is not None:
            try:
                if started:
                    trace.exchange('TRACE_STOP')
                    report['trace'] = trace.collect()
            except Exception as error:
                report.setdefault('cleanup_errors', []).append(str(error))
            finally:
                trace.close()
        if 'error' not in report and 'cleanup_errors' not in report:
            try:
                report['verification'] = verify_evidence(plan, report)
                report['result'] = 'passed'
            except Exception as error:
                report['error'] = {'type': type(error).__name__, 'message': str(error)}
        save()
    print(json.dumps({'result': report['result'], 'evidence': str(args.evidence)}), flush=True)
    return 0 if report['result'] == 'passed' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('plan', 'evidence', 'driver', 'plugin', 'source', 'trace-socket'):
        parser.add_argument('--' + name, type=Path, required=True)
    add_provenance_arguments(parser)
    raise SystemExit(run(parser.parse_args()))
