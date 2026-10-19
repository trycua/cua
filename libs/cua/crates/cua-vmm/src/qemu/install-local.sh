#!/bin/sh
set -eu
home=$1
for tool in ssh ssh-keygen dpkg-deb realpath; do
    command -v "$tool" >/dev/null || { echo "user-local QEMU requires $tool on PATH" >&2; exit 1; }
done
case $(uname -m) in
    x86_64) system=qemu-system-x86_64; packages='qemu-system-x86 seabios ipxe-qemu ovmf' ;;
    aarch64|arm64) system=qemu-system-aarch64; packages='qemu-system-arm qemu-efi-aarch64' ;;
    *) echo 'unsupported QEMU host architecture' >&2; exit 1 ;;
esac
mkdir -p "$home/bin"
stage=$(mktemp -d "$home/.qemu-XXXXXXXX")
old=$stage/old
cleanup() {
    if [ -d "$old" ] && [ ! -d "$home/qemu" ]; then mv "$old" "$home/qemu"; fi
    rm -rf "$stage"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$stage/lists/partial" "$stage/cache/archives/partial" "$stage/root"
set -- -o "Dir::State::Lists=$stage/lists" -o "Dir::Cache=$stage/cache" -o Debug::NoLocking=1
apt-get "$@" update
apt-get "$@" --download-only --reinstall --no-install-recommends -y install $packages qemu-system-common qemu-system-data qemu-utils
for deb in "$stage/cache/archives/"*.deb; do dpkg-deb -x "$deb" "$stage/root"; done
find "$stage/root" -type l -exec sh -c '
    root=$1; shift
    for link do
        target=$(readlink "$link")
        case $target in /*)
            if [ -e "$root$target" ]; then
                relative=$(realpath -s --relative-to="$(dirname "$link")" "$root$target")
                ln -snf "$relative" "$link"
            fi ;;
        esac
    done
' sh "$stage/root" {} +
wrapper() {
    root=$1; binary=$2; destination=$3
    quoted=$(printf '%s' "$root" | sed "s/'/'\\\\''/g")
    {
        printf "#!/bin/sh\nroot='%s'\n" "$quoted"
        printf '%s\n' 'export LD_LIBRARY_PATH="$root/usr/lib:$(find "$root/usr/lib" -mindepth 1 -maxdepth 1 -type d | paste -sd: -)${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"' 'export QEMU_MODULE_DIR="$(find "$root/usr/lib" -type d -name qemu | head -n 1)"'
        if [ "$binary" = qemu-img ]; then
            printf 'exec "$root/usr/bin/%s" "$@"\n' "$binary"
        else
            printf 'exec "$root/usr/bin/%s" -L "$root/usr/share/qemu" "$@"\n' "$binary"
        fi
    } > "$destination"
    chmod +x "$destination"
}
for binary in "$system" qemu-img; do
    wrapper "$stage/root" "$binary" "$stage/$binary"
    "$stage/$binary" --version
done
[ ! -e "$home/qemu" ] || mv "$home/qemu" "$old"
mv "$stage/root" "$home/qemu"
for binary in "$system" qemu-img; do
    wrapper "$home/qemu" "$binary" "$stage/$binary"
    mv "$stage/$binary" "$home/bin/$binary"
done
