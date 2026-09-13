"""Execute production cleanup decisions with fake transport/compositor state.

This is policy coverage, not evidence of Wayland delivery or app recovery. The
method bodies come from input_experiment.cpp so changing a production call site
changes the behavior under test; the fixture does not restate those decisions.
"""
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def method(source, name):
    match = re.search(r'^    void ' + name + r'\(.*?^    }', source, re.M | re.S)
    if not match:
        raise AssertionError(f'production method not found: {name}')
    return match.group()


def fixture(source):
    methods = '\n'.join(method(source, name) for name in (
        'invalidate', 'retire_grant', 'revoke', 'suspend', 'desktop_transition',
        'guard_targets', 'cancel_authority'))
    # Exercise the real refusal branches from both TARGET and action dispatch.
    for label, start, end in (
        ('target_refusal', 'const auto requested_cap =', 'const auto pid ='),
        ('action_refusal', 'const auto sequence = number(f[1]);', 'if (lease && Clock::now() >= expires)'),
    ):
        block = source.split(start, 1)[1].split(end, 1)[0]
        refusals = [line.strip() for line in block.splitlines()
                    if 'if (' in line and ('!available()' in line or '!layout_qualified()' in line)]
        if len(refusals) != 2:
            raise AssertionError(f'production refusal branches not found: {label}')
        methods += '\nvoid ' + label + '(Client& c) {\n' + '\n'.join(refusals) + '\n}'
    return (ROOT / 'tests/desktop_fault_policy_fixture.cpp').read_text().replace(
        '// PRODUCTION_METHODS', methods)


class DesktopFaultPolicyTests(unittest.TestCase):
    def test_production_desktop_fault_cleanup(self):
        source = (ROOT / 'src/input_experiment.cpp').read_text()
        # Fail if the tested guard is disconnected from the production timer.
        self.assertIn('guard_targets();', method(source, 'step'))
        compiler = shlex.split(os.environ.get('CXX', '')) or [
            shutil.which('clang++-18') or shutil.which('clang++') or 'c++']
        with tempfile.TemporaryDirectory(prefix='cua-desktop-fault-policy-') as directory:
            cpp = Path(directory) / 'policy.cpp'
            binary = Path(directory) / 'policy'
            cpp.write_text(fixture(source))
            build = subprocess.run([*compiler, '-std=c++20', '-Wall', '-Wextra',
                                    '-Wpedantic', '-Werror', '-I', str(ROOT / 'src'),
                                    str(cpp), '-o', str(binary)],
                                   capture_output=True, text=True, timeout=60)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
