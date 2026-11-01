"""Copy HEAD, apply only in scratch, build and run non-desktop checks."""
from pathlib import Path
import os
import shutil
import subprocess
import tarfile
import io

sidecar = Path(__file__).resolve().parent
repo = sidecar.parents[2]
scratch = Path('/home/netbos/.hermes/cache/scratch/cua3506-driver')
source = scratch / 'source'
source.mkdir(parents=True, exist_ok=True)
workspace = source / 'libs/cua-driver/rust'
if not (workspace / 'Cargo.toml').exists():
    archive = subprocess.check_output(['git', 'archive', 'HEAD', 'libs/cua-driver/rust', 'libs/cua-driver/wayland-helper'], cwd=repo)
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(source, filter='data')
patch = str(sidecar / 'driver.patch')
if subprocess.run(['git','apply','--reverse','--check',patch],cwd=source,capture_output=True).returncode:
    subprocess.run(['git','apply','--check',patch],cwd=source,check=True)
    subprocess.run(['git','apply',patch],cwd=source,check=True)
example = workspace / 'crates/cua-driver-sdk/examples/rfc3506.rs'
example.parent.mkdir(parents=True,exist_ok=True)
shutil.copyfile(sidecar/'sdk_harness.rs',example)
env = os.environ.copy()
env['RUSTFLAGS'] = '--cfg cua3506_prototype --check-cfg=cfg(cua3506_prototype)'
commands = [
    ['cargo','test','-p','platform-linux','--features','portal-input','prototype::tests','--lib'],
    ['cargo','build','-p','cua-driver-sdk','--features','portal-input','--example','rfc3506'],
    [str(workspace/'target/debug/examples/rfc3506'),'--self-check'],
]
with (scratch/'build.log').open('w') as log:
    for command in commands:
        print(' '.join(command),flush=True)
        result=subprocess.run(command,cwd=workspace,env=env,stdout=log,stderr=subprocess.STDOUT)
        if result.returncode:
            raise SystemExit(f'failed ({result.returncode}); see {scratch}/build.log')
print(f'PASS; binary: {workspace}/target/debug/examples/rfc3506; log: {scratch}/build.log')
