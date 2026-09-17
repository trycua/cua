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


@contextmanager
def fixture(port: int = 0):
    with FixtureServer(('127.0.0.1', port)) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f'http://127.0.0.1:{server.server_port}/'
        finally:
            server.shutdown()
            thread.join(timeout=5)


def verify(command: list[str], url: str, token: str, log: Path) -> dict:
    subprocess.run(command, cwd=BASE, check=True, timeout=180)
    events = [json.loads(line) for line in log.read_text().splitlines()]
    expected = {'event': 'outcome', 'outcome': 'verified', 'token': token}
    if not events or events[-1] != expected:
        raise RuntimeError('Runner did not report the expected verified outcome')
    with urlopen(url + 'state', timeout=2) as response:
        observed = json.load(response)
    if observed != {'submitted': token}:
        raise RuntimeError('Independent fixture state does not match the expected token')
    return {'outcome': 'verified', 'token': token, 'observed': observed}


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
    parser.add_argument('--output-dir', type=Path, required=True, help='new directory for evidence; existing paths are refused')
    args = parser.parse_args()
    if args.live:
        require_key()
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    summary = {'complete': False, 'live_requested': args.live, 'typescript_requested': args.typescript, 'checks': []}
    try:
        with fixture(args.port) as url:
            print(json.dumps({'event': 'fixture_ready', 'url': url}), flush=True)
            summary['fixture_url'] = url
            for language in (['python', 'typescript'] if args.typescript else ['python']):
                for provider in (['mock', 'live'] if args.live else ['mock']):
                    token = f'jev-guide-{provider}'
                    log = output / f'{language}-{provider}.jsonl'
                    command = runner_command(language, provider)
                    command += ['--fixture-url', url, '--token', token, '--max-steps', '4', '--log', str(log)]
                    result = {'language': language, 'provider': provider, **verify(command, url, token, log)}
                    summary['checks'].append(result)
                    print(json.dumps({'event': 'independently_verified', **result}), flush=True)
        summary['complete'] = True
    finally:
        (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps({'event': 'setup_complete', 'checks': len(summary['checks']), 'fixture_closed': True}), flush=True)


if __name__ == '__main__':
    main()
