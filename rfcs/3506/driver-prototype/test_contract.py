"""Structural checks for the separately applied prototype; never opens a desktop."""
from pathlib import Path
import subprocess
import tempfile
p = Path(__file__).with_name('driver.patch')
s = p.read_text() if p.exists() else ''
assert 'cua3506_prototype' in s, 'missing compile-time-isolated experiment'
assert 'GuardedPressKey' in s, 'missing worker-side guarded command'
assert 'GetIdentitySnapshot' in s, 'missing fresh identity lookup'
assert 'CLOCK_MONOTONIC' in s, 'missing shared clock'
assert 'available()' not in '\n'.join(x[1:] for x in s.splitlines() if x.startswith('+')), 'do not change capability availability'
assert 'op.check()?' in s and 'op.may_start' in s, 'missing final guard and ambiguity marker'
repo = p.resolve().parents[3]
relative = 'libs/cua-driver/rust/crates/platform-linux/src/tools/impl_.rs'
with tempfile.TemporaryDirectory() as directory:
    target = Path(directory) / relative
    target.parent.mkdir(parents=True)
    target.write_bytes(subprocess.check_output(['git', 'show', 'HEAD:' + relative], cwd=repo))
    subprocess.run(['git', 'apply', '--include=' + relative, str(p.resolve())], cwd=directory, check=True)
    patched = target.read_text()
    start = patched.rindex('impl Tool for PressKeyTool {')
    end = patched.find('impl Tool for ', start + 1)
    body = patched[start:end if end >= 0 else len(patched)]
    assert 'prototype::admitted(&args)' in body, 'guard must be on canonical PressKeyTool, not a sibling tool'
    assert patched.count('prototype::admitted(&args)') == 1, 'experiment must intercept only press_key'
print('sidecar structural contract: PASS (not a desktop/input test)')
