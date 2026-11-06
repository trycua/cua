#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# `gnucash` on PATH for the Flathub build, run outside bubblewrap on its own GNOME runtime. Needed only where
# flatpak cannot create user namespaces (a container); on a full VM `flatpak run org.gnucash.GnuCash` works and the
# pack can use a plain wrapper for it. Run as root after `flatpak install org.gnucash.GnuCash`.
set -euo pipefail
APPF=/var/lib/flatpak/app/org.gnucash.GnuCash/current/active/files
RTF=/var/lib/flatpak/runtime/org.gnome.Platform/x86_64/50/active/files
ln -sfn "$APPF" /app
cp -f "$RTF/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" "$(readlink -f "$APPF")/bin/gnucash-ld"
cat > /usr/local/bin/gnucash <<'SH'
#!/bin/bash
# GnuCash 5.17 (Flathub build, /app) on its GNOME 50 runtime's loader and libraries, outside bubblewrap.
RTF=/var/lib/flatpak/runtime/org.gnome.Platform/x86_64/50/active/files
export XDG_DATA_DIRS=/app/share:$RTF/share:/usr/share
export GIO_MODULE_DIR=$RTF/lib/x86_64-linux-gnu/gio/modules
export GDK_PIXBUF_MODULE_FILE=$RTF/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders.cache
export GTK_PATH=$RTF/lib/x86_64-linux-gnu/gtk-3.0
export GTK_EXE_PREFIX=$RTF GTK_DATA_PREFIX=$RTF
unset GTK_MODULES
export GNC_HOME=/app
exec -a gnucash /app/bin/gnucash-ld --library-path /app/lib:/app/lib/gnucash:$RTF/lib/x86_64-linux-gnu:$RTF/lib /app/bin/gnucash "$@"
SH
chmod 755 /usr/local/bin/gnucash
# glycin image loaders: the runtime's config names /usr/libexec/glycin-loaders/...; wrap each on the runtime loader.
for d in "$RTF"/libexec/glycin-loaders/*; do
  v=$(basename "$d"); mkdir -p "/usr/libexec/glycin-loaders/$v"
  for b in "$d"/*; do
    printf '#!/bin/sh\nexec %s/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2 --library-path %s/lib/x86_64-linux-gnu:%s/lib %s "$@"\n' \
      "$RTF" "$RTF" "$RTF" "$b" > "/usr/libexec/glycin-loaders/$v/$(basename "$b")"
    chmod 755 "/usr/libexec/glycin-loaders/$v/$(basename "$b")"
  done
done
echo GNUCASH_SHIM_OK
