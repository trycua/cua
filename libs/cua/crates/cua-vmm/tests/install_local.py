"""Hermetic installer regression: no apt network, sudo, or VM execution."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).parents[1] / 'src/qemu/install-local.sh'

class LocalInstall(unittest.TestCase):
    def test_install_and_failed_reinstall(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            home = base / "home with 'quote"
            tools = base / 'tools'
            tools.mkdir()
            def tool(name, body):
                path = tools / name
                path.write_text('#!/bin/sh\nset -eu\n' + body)
                path.chmod(0o755)
            tool('uname', 'echo x86_64\n')
            tool('find', '/usr/bin/find "$@" | sort\n')
            tool('apt-get', '''
case "$*" in *--download-only*)
    case "$*" in *--reinstall*--no-install-recommends*) ;; *) exit 20;; esac
    for arg do case $arg in Dir::Cache=*) cache=${arg#*=};; esac; done
    touch "$cache/archives/qemu.deb";;
esac
''')
            tool('dpkg-deb', '''
[ "${FAIL:-}" != yes ] || exit 21
root=$3
mkdir -p "$root/usr/bin" "$root/usr/lib/qemu" "$root/usr/lib/x86_64-linux-gnu/qemu" "$root/usr/share/qemu" "$root/usr/share/seabios"
touch "$root/usr/share/seabios/bios.bin"
ln -s /usr/share/seabios/bios.bin "$root/usr/share/qemu/bios.bin"
ln -s /host-only "$root/usr/share/qemu/host"
for binary in qemu-system-x86_64 qemu-img; do
    printf '#!/bin/sh\\nprintf "%%s\\\\n" "$QEMU_MODULE_DIR" "$LD_LIBRARY_PATH" "$@"\\n' > "$root/usr/bin/$binary"
    chmod +x "$root/usr/bin/$binary"
done
''')
            env = dict(os.environ, PATH=f'{tools}:/usr/bin:/bin')
            subprocess.run(['sh', str(SCRIPT), str(home)], env=env, check=True, capture_output=True)
            root = home / 'qemu'
            link = root / 'usr/share/qemu/bios.bin'
            self.assertFalse(os.readlink(link).startswith('/'))
            self.assertTrue(link.exists())
            self.assertEqual(os.readlink(root / 'usr/share/qemu/host'), '/host-only')
            for binary in ('qemu-system-x86_64', 'qemu-img'):
                output = subprocess.check_output([str(home / 'bin' / binary), 'a b', 'c"d'], env=env, text=True)
                self.assertEqual(output.splitlines()[0], str(root / 'usr/lib/x86_64-linux-gnu/qemu'))
                self.assertIn('a b\nc"d\n', output)
                self.assertEqual('\n-L\n' in output, binary != 'qemu-img')
            (root / 'sentinel').touch()
            result = subprocess.run(['sh', str(SCRIPT), str(home)], env=dict(env, FAIL='yes'), capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue((root / 'sentinel').exists())
            self.assertFalse(list(home.glob('.qemu-*')))
            subprocess.run(['sh', str(SCRIPT), str(home)], env=env, check=True, capture_output=True)
            self.assertFalse((root / 'sentinel').exists())

if __name__ == '__main__':
    unittest.main()
