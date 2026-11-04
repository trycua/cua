#!/bin/sh
# Prints the Arch Linux Archive day (YYYY/MM/DD) that satisfies every exact
# version cua-hyprland-plugin pins (glibc, hyprland, aquamarine, libgcc, ...).
#
# Omarchy's edge repo (pkgs.omarchy.org/edge) has no snapshots, and each
# cua-hyprland-plugin build pins the exact core/extra versions it was linked
# against. So the build follows the plugin: starting at the day the plugin was
# built and going back at most two weeks, the first archive day that serves
# every pinned version wins. Nothing here is rolling: the result is a dated
# archive snapshot, and packages stay signature-checked by pacman.
set -eu

edge=https://pkgs.omarchy.org/edge/x86_64
archive=https://archive.archlinux.org/repos
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL --retry 5 -o "$work/omarchy.db" "$edge/omarchy.db"
mkdir "$work/omarchy"
bsdtar -xf "$work/omarchy.db" -C "$work/omarchy"
desc=
for d in "$work"/omarchy/cua-hyprland-plugin-*/desc; do desc=$d; done
[ -f "$desc" ] || { echo "cua-hyprland-plugin is not in $edge" >&2; exit 1; }

# Exact pins (name=version), not ranges or soname provides.
awk '/^%DEPENDS%/{f=1;next} /^%/{f=0} f && /=/ && !/[<>]=/ && !/\.so=/' "$desc" >"$work/pins"
built=$(awk '/^%BUILDDATE%/{getline; print; exit}' "$desc")

# name version for every package in a repo database (one pass over the
# concatenated desc files: fast even under qemu-user).
versions() { # db-file out-file
    bsdtar -xOf "$1" '*/desc' |
        awk '/^%NAME%$/{getline; n=$0} /^%VERSION%$/{getline; print n" "$0}' >"$2"
}

day=0
while [ "$day" -le 14 ]; do
    date=$(date -u -d "@$((built - day * 86400))" +%Y/%m/%d)
    day=$((day + 1))
    versions "$work/omarchy.db" "$work/have"
    ok=1
    for repo in core extra; do
        curl -fsSL --retry 3 -o "$work/$repo.db" "$archive/$date/$repo/os/x86_64/$repo.db" 2>/dev/null || { ok=0; break; }
        versions "$work/$repo.db" "$work/$repo.txt"
        cat "$work/$repo.txt" >>"$work/have"
    done
    [ "$ok" = 1 ] || continue
    missing=$(while IFS= read -r pin; do
        name=${pin%%=*}
        want=${pin#*=}
        # A pin without a pkgrel (foo=1.2) matches any foo-1.2-N.
        awk -v n="$name" -v v="$want" '$1 == n && ($2 == v || index($2, v "-") == 1) {found=1} END{exit !found}' "$work/have" \
            || echo "$pin"
    done <"$work/pins")
    if [ -z "$missing" ]; then
        echo "$date"
        exit 0
    fi
done
echo "no Arch archive day in the two weeks before cua-hyprland-plugin's build satisfies its pins:" >&2
cat "$work/pins" >&2
exit 1
