#!/bin/zsh
set -euo pipefail

# Install the exact offline Node/Electron toolchain used by the disposable
# macOS benchmark seed. Both archives are downloaded and checksum-verified by
# the host before this script is copied into the guest.

node_sha256=61130f394c1630d211dd50aecc4353d379480f36d3ac913cd85dbba1aed585c6
electron_sha256=827f9f182566f46846377575b51c547b9926b111637313a373b6f717462aebac
node_binary_sha256=18e387c90ab8a8400183e8bdd396376e1e875b91b4c874b894dcade7b35bf572
electron_binary_sha256=1af684f056a8eb13e49fbd677072e437316086b076e3b9b92de3ddb343edc5b1
node_root=/opt/cdb/node-v22.23.2-darwin-arm64
electron_root=/opt/cdb/electron-43.4.0

if [[ ${EUID} -ne 0 ]]; then
  print -u2 "install-cdb-electron-toolchain: root is required"
  exit 64
fi
if [[ $# -ne 2 || $1 != /* || $2 != /* || ! -f $1 || ! -f $2 || -L $1 || -L $2 ]]; then
  print -u2 "usage: install-cdb-electron-toolchain.sh /absolute/node.tar.gz /absolute/electron.zip"
  exit 64
fi
if [[ $(/usr/bin/uname -m) != arm64 ]]; then
  print -u2 "install-cdb-electron-toolchain: arm64 guest required"
  exit 65
fi

actual_node=$(/usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}')
actual_electron=$(/usr/bin/shasum -a 256 "$2" | /usr/bin/awk '{print $1}')
if [[ $actual_node != $node_sha256 || $actual_electron != $electron_sha256 ]]; then
  print -u2 "install-cdb-electron-toolchain: archive digest mismatch"
  exit 66
fi

/usr/bin/install -d -o root -g wheel -m 0755 /opt/cdb
if [[ -L $node_root || -L $electron_root ]]; then
  print -u2 "install-cdb-electron-toolchain: install root cannot be a symlink"
  exit 67
fi
if [[ ! -d $node_root ]]; then
  /usr/bin/tar -xzf "$1" -C /opt/cdb
fi
if [[ ! -d $electron_root/Electron.app ]]; then
  /usr/bin/install -d -o root -g wheel -m 0755 "$electron_root"
  /usr/bin/ditto -x -k "$2" "$electron_root"
fi

node_binary=$node_root/bin/node
electron_binary=$electron_root/Electron.app/Contents/MacOS/Electron
[[ -x $node_binary && -x $electron_binary ]]
[[ $($node_binary --version) == v22.23.2 ]]
actual_node_binary=$(/usr/bin/shasum -a 256 "$node_binary" | /usr/bin/awk '{print $1}')
actual_electron_binary=$(/usr/bin/shasum -a 256 "$electron_binary" | /usr/bin/awk '{print $1}')
if [[ $actual_node_binary != $node_binary_sha256 || $actual_electron_binary != $electron_binary_sha256 ]]; then
  print -u2 "install-cdb-electron-toolchain: installed binary digest mismatch"
  exit 68
fi

/usr/bin/install -d -o root -g wheel -m 0755 /usr/local/bin
for command in node npm npx; do
  /bin/ln -sfn "$node_root/bin/$command" "/usr/local/bin/$command"
done

/usr/sbin/chown -R root:wheel "$node_root" "$electron_root"
/bin/chmod -R go-w "$node_root" "$electron_root"
/usr/bin/shasum -a 256 "$node_binary" "$electron_binary"
