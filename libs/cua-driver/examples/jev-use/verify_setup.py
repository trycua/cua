from __future__ import annotations

import argparse
import getpass
import json
import os
import subprocess
import sys
import threading
from contextlib import contextmanager
from pathlib import Path
from urllib.request import urlopen

from fixture_server import FixtureServer


BASE = Path(__file__).resolve().parent


VISUAL_STATUSES = ('ok', 'not_installed', 'error', 'unavailable')
SUBMIT_IDS = ('submit-form', 'submit-form-foreground')


@contextmanager
def fixture(port: int = 0, *, visual: bool = False):
    with FixtureServer(('127.0.0.1', port), visual=visual) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f'http://127.0.0.1:{server.server_port}/'
        finally:
            server.shutdown()
            thread.join(timeout=5)


def acted_path(events: list[dict]) -> dict:
    """Report which Driver path acted, from the runner's redacted JSONL log.

    ``submit_tool`` is ``browser_click`` for the DOM page-structure path, ``click``
    for the capture-bound visual path, or ``None`` when nothing submitted.
    """
    steps = [event for event in events if event.get('event') == 'step']
    submits = [
        event for event in steps
        if event.get('candidate') in SUBMIT_IDS and not event.get('action_error')
    ]
    statuses = [
        event['visual']['status'] if isinstance(event.get('visual'), dict) else None
        for event in events
        if event.get('event') == 'step' or 'visual' in event
    ]
    tool = submits[-1].get('tool') if submits else None
    path = {'browser_click': 'page_structure', 'click': 'visual'}.get(tool) if tool else None
    return {
        'submit_tool': tool,
        'acted_path': path,
        'submit_delivery_mode': submits[-1].get('delivery_mode') if submits else None,
        'visual_statuses': statuses,
        'escalations': [event['escalation'] for event in steps if isinstance(event.get('escalation'), dict)],
    }


def verify(
    command: list[str],
    url: str,
    token: str,
    log: Path,
    *,
    require_visual: bool = False,
    expect_visual_status: str | None = None,
    visual_fixture: bool = False,
) -> dict:
    """Run one runner and independently verify what it did.

    With ``expect_visual_status`` other than ``ok`` on the visual fixture, no
    page-structure Submit exists, so the correct result is a logged fallback
    that never submits and never claims success.
    """
    expect_fallback = visual_fixture and expect_visual_status not in (None, 'ok')
    completed = subprocess.run(command, cwd=BASE, check=False, timeout=180)
    events = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    path = acted_path(events)
    if expect_visual_status is not None:
        # Steps that skipped the parse because the page structure already offered
        # an action are not visual attempts; at least one attempt must exist.
        attempted = [status for status in path['visual_statuses'] if status != 'skipped']
        if not attempted or any(status != expect_visual_status for status in attempted):
            raise RuntimeError(
                f'Runner did not log visual status {expect_visual_status!r} on every step: '
                f'{path["visual_statuses"]}'
            )
    with urlopen(url + 'state', timeout=2) as response:
        observed = json.load(response)
    if expect_fallback:
        if path['submit_tool'] is not None:
            raise RuntimeError('Runner submitted without a usable visual observation')
        if observed != {'submitted': None}:
            raise RuntimeError('Independent fixture state shows an unexpected submission')
        final = events[-1] if events else {}
        if final.get('event') != 'outcome' or final.get('outcome') == 'verified':
            raise RuntimeError('Runner did not report a non-verified fallback outcome')
        return {'outcome': final['outcome'], 'token': token, 'observed': observed, **path}
    if completed.returncode != 0:
        raise subprocess.CalledProcessError(completed.returncode, command)
    expected = {'event': 'outcome', 'outcome': 'verified', 'token': token}
    if not events or events[-1] != expected:
        raise RuntimeError('Runner did not report the expected verified outcome')
    if observed != {'submitted': token}:
        raise RuntimeError('Independent fixture state does not match the expected token')
    if require_visual and path['acted_path'] != 'visual':
        raise RuntimeError(
            'Visual path was required but the runner submitted with '
            f'{path["submit_tool"] or "no action"}; visual statuses: {path["visual_statuses"]}'
        )
    return {'outcome': 'verified', 'token': token, 'observed': observed, **path}


def require_key() -> None:
    if os.environ.get('TYPESAFE_API_KEY', '').strip():
        return
    if not sys.stdin.isatty():
        raise SystemExit('Human prerequisite: provision TYPESAFE_API_KEY before an unattended live run')
    key = getpass.getpass('TypeSafe API key: ').strip()
    if not key:
        raise SystemExit('No key supplied')
    os.environ['TYPESAFE_API_KEY'] = key


def runner_command(language: str, provider: str) -> list[str]:
    if language == 'python':
        return [sys.executable, 'python/run.py', '--provider', provider]
    return ['node', '--import', 'tsx', 'typescript/run.ts', '--provider', provider]


def main() -> None:
    parser = argparse.ArgumentParser(description='Verify Jev setup with an owned, automatically cleaned-up fixture.')
    parser.add_argument('--live', action='store_true', help='also verify live Jev; requires a TypeSafe key')
    parser.add_argument('--typescript', action='store_true', help='also verify the installed TypeScript agent')
    parser.add_argument('--port', type=int, default=0, help='fixture port; defaults to an unused loopback port')
    parser.add_argument('--max-steps', type=int, default=4, help='maximum decisions per runner')
    parser.add_argument('--output-dir', type=Path, required=True, help='new directory for evidence; existing paths are refused')
    parser.add_argument('--visual-fixture', action='store_true',
                        help='serve a Submit control with no DOM button ref so only the capture-bound visual click can submit')
    parser.add_argument('--require-visual-path', action='store_true',
                        help='fail unless every runner submitted through the capture-bound visual click')
    parser.add_argument('--expect-visual-status', choices=VISUAL_STATUSES,
                        help='fail unless every step that attempted a visual observation logs this status; '
                             'with --visual-fixture and a non-ok status, '
                             'require a logged fallback that never submits')
    args = parser.parse_args()
    if args.require_visual_path and args.expect_visual_status not in (None, 'ok'):
        parser.error('--require-visual-path needs a working visual observation')
    if args.live:
        require_key()
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    summary = {
        'complete': False,
        'live_requested': args.live,
        'typescript_requested': args.typescript,
        'fixture': 'visual' if args.visual_fixture else 'default',
        'visual_path_required': args.require_visual_path,
        'expected_visual_status': args.expect_visual_status,
        'checks': [],
    }
    try:
        with fixture(args.port, visual=args.visual_fixture) as url:
            print(json.dumps({'event': 'fixture_ready', 'url': url}), flush=True)
            summary['fixture_url'] = url
            for language in (['python', 'typescript'] if args.typescript else ['python']):
                for provider in (['mock', 'live'] if args.live else ['mock']):
                    token = f'jev-guide-{provider}'
                    log = output / f'{language}-{provider}.jsonl'
                    command = runner_command(language, provider)
                    command += ['--fixture-url', url, '--token', token, '--max-steps', str(args.max_steps), '--log', str(log)]
                    result = {
                        'language': language,
                        'provider': provider,
                        **verify(
                            command,
                            url,
                            token,
                            log,
                            require_visual=args.require_visual_path,
                            expect_visual_status=args.expect_visual_status,
                            visual_fixture=args.visual_fixture,
                        ),
                    }
                    summary['checks'].append(result)
                    print(json.dumps({'event': 'independently_verified', **result}), flush=True)
        summary['complete'] = True
    finally:
        (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps({'event': 'setup_complete', 'checks': len(summary['checks']), 'fixture_closed': True}), flush=True)


if __name__ == '__main__':
    main()
