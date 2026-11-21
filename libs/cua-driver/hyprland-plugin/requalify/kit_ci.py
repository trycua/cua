#!/usr/bin/env python3
"""Measure, build and qualify the Cua Hyprland profile kit for one Omarchy channel.

Runs as root inside a disposable Arch container (see ci-hyprland-plugin-kit.yml).
For one channel it installs the channel's exact packages, then:

  1. measures the compositor, headers, compiler and ABI runtime into a candidate
     profile for the reviewed source archive (profile_measure.py);
  2. compares that candidate with the committed profile, when there is one, and
     reports whether the reviewed profile still describes the channel
     (`profile-unchanged`) or the channel moved (`rebuild`);
  3. bundles the kit from the candidate with the committed tooling
     (profile_bundle.py), builds the package with makepkg as an ordinary user and
     qualifies install, remove, reinstall and dependency refusal in isolated
     ALPM roots (lifecycle.py);
  4. loads the packaged module into a headless Hyprland and runs the background
     input smoke (headless.py) through qualify.session.

The result is CI evidence for the kit tooling and the profile. It is not native
certification, and nothing here publishes a package or release.
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import time

HERE = Path(__file__).resolve().parent
RELEASE = HERE.parent / "packaging" / "release"
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(RELEASE))

import profile_measure  # noqa: E402
import profile_verify as verify  # noqa: E402
import qualify  # noqa: E402

# makepkg and the isolated ALPM roots need an ordinary user with sudo.
KIT_PACKAGES = ("base-devel", "git", "sudo", "zstd", "libarchive")
BUILDER = "builder"


def sha256_file(path):
    return qualify.sha256_file(path)


def source_manifest_sha256(archive):
    stem = archive.name.removesuffix(".tar.gz")
    with tarfile.open(archive, "r:gz") as handle:
        return verify.sha256(handle.extractfile(f"{stem}/SOURCE-PROVENANCE.json").read())


def prepare_builder(log):
    qualify.run(["pacman", "-S", "--noconfirm", "--needed", *KIT_PACKAGES], log)
    if subprocess.run(["id", BUILDER], capture_output=True).returncode:
        qualify.run(["useradd", "-m", "-u", "1001", BUILDER])
    Path("/etc/sudoers.d/builder").write_text(f"{BUILDER} ALL=(ALL) NOPASSWD: ALL\n")
    Path("/etc/sudoers.d/builder").chmod(0o440)
    qualify.run(["git", "config", "--system", "--add", "safe.directory", "*"])


def as_builder(command, log, check=True, timeout=3600):
    return qualify.run(["runuser", "-u", BUILDER, "--", "env", "HOME=/home/" + BUILDER, "LC_ALL=C",
                        "PATH=/usr/bin:/usr/sbin", *map(str, command)], log, check=check, timeout=timeout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--channel", required=True, choices=sorted(qualify.channels.CHANNELS))
    parser.add_argument("--repo", type=Path, required=True, help="checkout of the tooling revision (git)")
    parser.add_argument("--tooling-revision", required=True)
    parser.add_argument("--archive", type=Path, required=True, help="reviewed Driver plugin source archive")
    parser.add_argument("--revision", required=True, help="source revision named by the archive")
    parser.add_argument("--driver-version", required=True)
    parser.add_argument("--profile-id", required=True)
    parser.add_argument("--kit-version", required=True)
    parser.add_argument("--package-release", required=True, type=int)
    parser.add_argument("--reviewed", type=Path, help="committed profile to compare the measurement with")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--skip-session", action="store_true")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    args.out.chmod(0o777)
    log = args.out / "kit.log"
    record = {"schema": 1, "channel": args.channel, "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "driver_version": args.driver_version, "source_revision": args.revision, "status": "fail", "steps": {}}
    try:
        job = {"channel": args.channel, "needs_omarchy_repo": args.channel != "arch"}
        qualify.setup_pacman(job, args.out / "pacman.log")
        prepare_builder(args.out / "pacman-kit.log")
        record["measured"] = qualify.measure(job)
        record["steps"]["environment"] = "pass"

        # 1. Candidate profile. The reviewer's trust roots are the archive digest, which the workflow
        #    checks against the release's published digest, and the manifest digest inside it.
        archive = args.archive
        source = {"revision": args.revision, "driver_version": args.driver_version,
                  "archive_sha256": sha256_file(archive), "manifest_sha256": source_manifest_sha256(archive)}
        candidate = profile_measure.measure(args.profile_id, args.kit_version, args.package_release, source,
                                            archive, Path("/usr/bin/g++"))
        candidate_path = args.out / "candidate-profile.json"
        candidate_path.write_bytes(verify.json_bytes(candidate))
        record["steps"]["measure"] = "pass"
        # Artifact downloads can be rate limited; the candidate is public data, so keep it in the log too.
        print("=== candidate-profile.json begin\n" + candidate_path.read_text() + "=== candidate-profile.json end", flush=True)

        # 2. Is the committed profile still this channel?
        if args.reviewed:
            verdict, reasons = profile_measure.reuse_decision(profile_measure.load(args.reviewed), candidate)
            record["reviewed_profile"] = {"file": args.reviewed.name, "verdict": verdict, "reasons": reasons}

        # 3. Kit from the committed tooling, then the package lifecycle as an ordinary user.
        kit_dir = args.out / "kit-bundle"
        qualify.run([sys.executable, str(RELEASE / "profile_bundle.py"), "--repo", str(args.repo),
                     "--tooling-revision", args.tooling_revision, "--profile", str(candidate_path),
                     "--source-archive", str(archive), "--output", str(kit_dir)], log)
        kit_archive = next(kit_dir.glob("*.tar.gz"))
        extracted = args.out / "kit"
        extracted.mkdir()
        with tarfile.open(kit_archive, "r:gz") as handle:
            handle.extractall(extracted, filter="data")
        kit_sha = verify.sha256((extracted / "KIT-PROVENANCE.json").read_bytes())
        shutil.chown(extracted, BUILDER, BUILDER)
        for path in extracted.iterdir():
            shutil.chown(path, BUILDER, BUILDER)
        record["kit"] = {"archive": kit_archive.name, "archive_sha256": sha256_file(kit_archive),
                         "kit_provenance_sha256": kit_sha}
        evidence = args.out / "lifecycle"
        code, _ = as_builder([sys.executable, str(extracted / "lifecycle.py"), "--kit", extracted,
                              "--revision", args.revision, "--driver-version", args.driver_version,
                              "--kit-sha256", kit_sha, "--output", evidence], args.out / "lifecycle.log", check=False)
        record["steps"]["lifecycle"] = "pass" if code == 0 and (evidence / "RESULT.json").is_file() else "fail"
        if record["steps"]["lifecycle"] == "pass":
            packages = [p for p in (evidence / "kit").glob("cua-hyprland-plugin-*.pkg.tar.*") if p.suffix != ".sig"]
            package = packages[0]
            record["package"] = {"file": package.name, "sha256": sha256_file(package)}
            module = args.out / "packaged" / "cua-hyprland-plugin.so"
            module.parent.mkdir()
            with module.open("wb") as handle:
                handle.write(subprocess.check_output(["bsdtar", "-xOf", str(package), "usr/lib/cua/hyprland/cua-hyprland-plugin.so"]))
            record["module_sha256"] = sha256_file(module)
            # 4. The packaged bytes, loaded and driven.
            if args.skip_session:
                record["load"] = record["smoke"] = {"status": "unavailable", "reason": "session skipped"}
            else:
                record["load"], record["smoke"] = qualify.session(module, args.out)
    except Exception as error:  # Recorded as a failure of this channel, never swallowed.
        record["error"] = f"{type(error).__name__}: {error}"
        detail = getattr(error, "output", "") or ""
        # Environment facts that explain a profile_verify refusal on a new Hyprland build.
        for command in (["pkgconf", "--cflags", "hyprland"], ["sh", "-c", "pkgconf --cflags-only-I hyprland | tr ' ' '\\n' | sed 's/^-I//' | xargs -r ls -ld"]):
            probe = subprocess.run(command, capture_output=True, text=True)
            print(f"--- {' '.join(command[:3])}\n{probe.stdout}{probe.stderr}", file=sys.stderr)
        print(f"error: {record['error']}\n{detail[-3000:]}", file=sys.stderr)
        for name in ("pacman.log", "kit.log"):
            if (args.out / name).is_file():
                print(f"--- tail of {name}\n" + (args.out / name).read_text()[-3000:], file=sys.stderr)
    unavailable = {"status": "unavailable", "reason": "not reached"}
    record.setdefault("load", dict(unavailable))
    record.setdefault("smoke", dict(unavailable))
    ok = record["steps"].get("lifecycle") == "pass" and "error" not in record
    if ok and record["load"]["status"] == "pass" and record["smoke"]["status"] == "pass":
        record["status"] = "pass"
    elif ok and "fail" not in (record["load"]["status"], record["smoke"]["status"]):
        record["status"] = "build-only"
    else:
        record["status"] = "fail"
    record["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    (args.out / "result.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    print(json.dumps({k: record.get(k) for k in ("status", "steps", "reviewed_profile", "load", "smoke", "error")}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
