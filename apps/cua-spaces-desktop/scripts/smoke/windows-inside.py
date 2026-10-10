# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs inside the container; see windows.sh.
import glob, json, os, re, struct, subprocess, sys, tempfile
import pefile

VERSION = os.environ["VERSION"]
MACHINE = {0x8664: "x64", 0xAA64: "arm64"}
PAYLOAD = {"app-64.7z": "x64", "app-arm64.7z": "arm64"}
failed = False

def check(name, ok, detail=""):
    global failed
    failed |= not ok
    print(f"{'PASS' if ok else 'FAIL'} {name}{f' ({detail})' if detail else ''}")

def sh(*args):
    return subprocess.run(args, capture_output=True, text=True)

def asar_files(path):
    with open(path, "rb") as f:
        f.read(12)
        size = struct.unpack("<I", f.read(4))[0]
        header = json.loads(f.read(size))
    files = []
    def walk(node, prefix):
        for k, v in node.get("files", {}).items():
            walk(v, f"{prefix}{k}/") if "files" in v else files.append(prefix + k)
    walk(header, "")
    return files

def version_info(pe):
    info = {}
    for fi in getattr(pe, "FileInfo", []) or []:
        for entry in fi:
            for st in getattr(entry, "StringTable", []):
                info.update({k.decode(): v.decode() for k, v in st.entries.items()})
    return info

def signature(path):
    # The PE security directory holds the Authenticode blob; empty means unsigned.
    pe = pefile.PE(path, fast_load=True)
    sec = pe.OPTIONAL_HEADER.DATA_DIRECTORY[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_SECURITY"]]
    return f"Authenticode blob, {sec.Size} bytes" if sec.Size else "unsigned"

for installer in sorted(glob.glob(f"/dist/Cua-Spaces-Setup-{VERSION}*.exe")):
    name = os.path.basename(installer)
    print(f"\n== {name} ({os.path.getsize(installer) / 1e6:.1f} MB)")
    listing = sh("7z", "l", installer).stdout
    check("NSIS 3 Unicode installer", "Type = Nsis" in listing and "NSIS-3 Unicode" in listing)
    payloads = [p for p in PAYLOAD if f"$PLUGINSDIR/{p}" in listing]
    want = {"x64": ["app-64.7z"], "arm64": ["app-arm64.7z"]}.get(
        (re.search(r"-(x64|arm64)\.exe$", name) or [None, "both"])[1], ["app-64.7z", "app-arm64.7z"])
    check("payload per arch", sorted(payloads) == sorted(want), ", ".join(payloads))
    check("uninstaller present", "Uninstall Cua Spaces.exe" in listing)
    check("WinShell plugin (sets the AppUserModelID on shortcuts)", "WinShell.dll" in listing)
    print(f"     installer signature: {signature(installer)}")
    with tempfile.TemporaryDirectory() as t:
        sh("7z", "x", "-y", f"-o{t}", installer)
        for p in payloads:
            arch = PAYLOAD[p]
            app = os.path.join(t, arch)
            sh("7z", "x", "-y", f"-o{app}", os.path.join(t, "$PLUGINSDIR", p))
            exe = os.path.join(app, "Cua Spaces.exe")
            print(f"  -- {p} -> {arch}")
            check(f"[{arch}] Cua Spaces.exe present", os.path.exists(exe))
            if not os.path.exists(exe):
                continue
            pe = pefile.PE(exe)
            check(f"[{arch}] PE machine", MACHINE.get(pe.FILE_HEADER.Machine) == arch, MACHINE.get(pe.FILE_HEADER.Machine, hex(pe.FILE_HEADER.Machine)))
            check(f"[{arch}] GUI subsystem", pe.OPTIONAL_HEADER.Subsystem == 2)
            vi = version_info(pe)
            check(f"[{arch}] ProductName Cua Spaces", vi.get("ProductName") == "Cua Spaces", vi.get("ProductName"))
            check(f"[{arch}] FileVersion {VERSION}", vi.get("FileVersion", "").startswith(VERSION), vi.get("FileVersion"))
            check(f"[{arch}] CompanyName set", bool(vi.get("CompanyName")), vi.get("CompanyName"))
            icons = [e for e in pe.DIRECTORY_ENTRY_RESOURCE.entries if e.id == pefile.RESOURCE_TYPE["RT_GROUP_ICON"]]
            check(f"[{arch}] icon resource", bool(icons))
            print(f"     exe signature: {signature(exe)}")
            res = os.path.join(app, "resources")
            for rel in ["app.asar", "web/index.html", "tray/icon.ico", "app-update.yml", "elevate.exe"]:
                check(f"[{arch}] resources/{rel}", os.path.exists(os.path.join(res, rel)))
            maps = glob.glob(os.path.join(res, "web", "**", "*.map"), recursive=True)
            check(f"[{arch}] no source maps in web/", not maps, f"{len(maps)} found")
            feed = open(os.path.join(res, "app-update.yml")).read()
            check(f"[{arch}] update feed: GitHub placeholder, channel latest",
                  "provider: github" in feed and "repo: cua-spaces-releases" in feed and "channel: latest" in feed)
            files = asar_files(os.path.join(res, "app.asar"))
            for rel in ["package.json", "dist-electron/boot.cjs", "dist-electron/main.cjs", "dist-electron/preload.cjs"]:
                check(f"[{arch}] app.asar has {rel}", rel in files)
            check(f"[{arch}] app.asar ships electron-updater", any(f.startswith("node_modules/electron-updater/") for f in files))
            check(f"[{arch}] no .map in app.asar", not any(f.endswith(".map") for f in files))
            locales = os.listdir(os.path.join(app, "locales"))
            check(f"[{arch}] locales trimmed to en", all(l.startswith("en") for l in locales), ", ".join(sorted(locales)))

print("\nSMOKE FAILED" if failed else "\nSMOKE OK")
sys.exit(1 if failed else 0)
