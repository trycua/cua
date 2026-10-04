#!/usr/bin/env python3
"""Export a reviewed, checksum-pinned downstream PKGBUILD for an existing kit.

The output is a separately reviewed wrapper, not the byte-identical inner recipe.
This performs no download, publication, native certification, or signing.
"""

import argparse
import io
from pathlib import Path
import re
import tarfile
import tempfile

import profile_verify as verify

HERE = Path(__file__).resolve().parent
INVENTORY = set(verify.TOOLING) | {
    "PROFILE.json", "KIT-PROVENANCE.json", "SOURCE-PROVENANCE.json",
    "PKGBUILD", "SHA256SUMS",
}


def archive_payload(data):
    """Read the flat kit without extracting or importing anything from it."""
    payload = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            verify.require(member.isfile() and not member.issparse() and not member.pax_headers,
                           "outer kit contains a nonregular or extended member")
            verify.require(member.name in INVENTORY or re.fullmatch(
                r"cua-hyprland-plugin-[0-9]+\.[0-9]+\.[0-9]+-[0-9a-f]{40}\.tar\.gz", member.name),
                "unsafe or unexpected outer kit path")
            verify.require(member.name not in payload, "duplicate outer kit member")
            payload[member.name] = archive.extractfile(member).read()
    verify.require("PROFILE.json" in payload, "outer kit inventory mismatch")
    profile = verify.validate_profile(verify.read_json(payload["PROFILE.json"]))
    verify.require(set(payload) == INVENTORY | {verify.source_stem(profile) + ".tar.gz"}, "outer kit inventory mismatch")
    expected_sums = "".join(f"{verify.sha256(data)}  {name}\n" for name, data in sorted(payload.items())
                            if name != "SHA256SUMS").encode()
    verify.require(payload["SHA256SUMS"] == expected_sums, "outer kit SHA256SUMS mismatch")
    return payload


def reviewed_kit(archive, expected_sha):
    verify.hash_value(expected_sha)
    verify.require(archive.is_file() and not archive.is_symlink(), "outer archive must be a regular file")
    data = archive.read_bytes()
    verify.require(verify.sha256(data) == expected_sha, "outer archive checksum mismatch")
    payload = archive_payload(data)
    # Only local reviewed code executes. All scripts/templates must match this checkout.
    for name in verify.TOOLING:
        verify.require(payload[name] == (HERE / name).read_bytes(), f"kit differs from local reviewed tooling: {name}")
    with tempfile.TemporaryDirectory(prefix="cua-profile-download-") as temporary:
        kit = Path(temporary)
        for name, content in payload.items():
            (kit / name).write_bytes(content)
        profile, provenance = verify.verify_kit(kit, verify.sha256(payload["KIT-PROVENANCE.json"]), complete=True)
        verify.require(payload["KIT-PROVENANCE.json"] == verify.json_bytes(provenance),
                       "kit provenance must match the recipe's canonical checksum")
        verify.source_manifest(payload["SOURCE-PROVENANCE.json"], profile)
        stem = verify.source_stem(profile)
        verify.verify_archive(kit / (stem + ".tar.gz"), profile)
        with tarfile.open(fileobj=io.BytesIO(payload[stem + ".tar.gz"]), mode="r:gz") as source:
            verify.require(all(member.isfile() and not member.issparse() and not member.pax_headers for member in source),
                           "source archive contains a nonregular or extended member")
    expected_name = (f"{stem}-profile-{profile['profile_id']}-kit-{profile['kit_version']}"
                     f"-{provenance['profile_sha256']}-{provenance['tooling_revision']}.tar.gz")
    verify.require(archive.name == expected_name, "outer archive filename does not match kit identity")
    return payload, profile, provenance


def replace_once(text, old, new):
    verify.require(text.count(old) == 1, "reviewed recipe adaptation anchor changed or duplicated")
    return text.replace(old, new, 1)


# This wrapper-owned code runs before any downloaded Python is executed. Its
# pinned inventory also protects the extracted kit in --noextract/repackage runs.
# Read each archive into memory once, so validation and extraction use the same bytes.
DOWNLOAD_CHECK = r'''
_verify_download() {
  python3 -I - "$SRCDEST/$_download_name" "$_download_sha256" "$srcdir" "$1" <<'CUA_DOWNLOAD_PY'
import hashlib
import io
from pathlib import Path, PurePosixPath
import sys
import tarfile

expected = @MEMBER_HASHES@
stem = '@STEM@'

def require(condition, message):
    if not condition:
        raise SystemExit(message)

def digest(data):
    return hashlib.sha256(data).hexdigest()

archive, checksum, srcdir, mode = sys.argv[1:]
archive, srcdir = Path(archive), Path(srcdir)
require(mode in {'check', 'extract'}, 'invalid kit verification mode')
require(archive.is_file() and not archive.is_symlink(), 'outer archive must be a regular file')
data = archive.read_bytes()
require(digest(data) == checksum, 'outer archive checksum mismatch')
payload = {}
with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as contents:
    for member in contents:
        require(member.isfile() and not member.issparse() and not member.pax_headers,
                'nonregular outer kit member')
        require(member.name in expected and member.name not in payload, 'outer kit inventory mismatch')
        content = contents.extractfile(member).read()
        require(digest(content) == expected[member.name], 'outer kit member checksum mismatch')
        payload[member.name] = content
require(payload.keys() == expected.keys(), 'outer kit inventory mismatch')
require(srcdir.is_dir() and not srcdir.is_symlink(), 'srcdir must be a real directory')
kit, source = srcdir / 'cua-profile-kit', srcdir / stem
if mode == 'extract':
    require(not kit.exists() and not kit.is_symlink() and not source.exists() and not source.is_symlink(),
            'prepare requires fresh kit and source destinations; use a clean srcdir')
    source_payload = {}
    with tarfile.open(fileobj=io.BytesIO(payload[stem + '.tar.gz']), mode='r:gz') as contents:
        for member in contents:
            require(member.isfile() and not member.issparse() and not member.pax_headers and
                    member.name.startswith(stem + '/'), 'invalid source member')
            name = member.name[len(stem) + 1:]
            path = PurePosixPath(name)
            require(name and path.as_posix() == name and not path.is_absolute() and
                    '..' not in path.parts and '\\' not in name and name not in source_payload,
                    'unsafe or duplicate source path')
            source_payload[name] = contents.extractfile(member).read()
    kit.mkdir()
    source.mkdir()
    for name, content in payload.items():
        (kit / name).write_bytes(content)
    for name, content in source_payload.items():
        destination = source / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
require(kit.is_dir() and not kit.is_symlink(), 'kit must be a real directory')
require({path.name for path in kit.iterdir()} == expected.keys(), 'extracted kit inventory mismatch')
for name, checksum in expected.items():
    path = kit / name
    require(path.is_file() and not path.is_symlink() and digest(path.read_bytes()) == checksum,
            'extracted kit checksum mismatch: ' + name)
CUA_DOWNLOAD_PY
}
'''


def adapt_recipe(payload, profile, provenance, archive_name, expected_sha, download_url):
    original = payload["PKGBUILD"].decode()
    recipe = original
    # Full rendering was verified above; refuse template drift even in reviewed tooling.
    verify.require(recipe.count("$startdir") == 8, "reviewed recipe startdir references changed")
    recipe = recipe.replace("$startdir", "$srcdir/cua-profile-kit")
    source = ('source=("${_stem}.tar.gz" \'KIT-PROVENANCE.json\' \'PROFILE.json\' \'profile_verify.py\')\n'
              f"sha256sums=('{profile['source']['archive_sha256']}' '{verify.sha256(verify.json_bytes(provenance))}' "
              f"'{provenance['profile_sha256']}' '{provenance['tooling_files']['profile_verify.py']}')")
    replacement = (f"_download_name='{archive_name}'\n_download_sha256='{expected_sha}'\n"
                   f"source=('{download_url}')\nnoextract=(\"$_download_name\")\nsha256sums=('{expected_sha}')")
    recipe = replace_once(recipe, source, replacement)
    recipe = replace_once(recipe, "_verify() {\n", "_verify() {\n"
                          "  printf '%s  %s\\n' \"$_download_sha256\" \"$SRCDEST/$_download_name\" | sha256sum -c - || return 1\n"
                          "  _verify_download check || return 1\n")
    recipe = replace_once(recipe, "prepare() {\n  _verify\n}",
                          "prepare() {\n  _verify_download extract || return 1\n  _verify\n}")
    runtime = DOWNLOAD_CHECK.replace("@MEMBER_HASHES@", repr({name: verify.sha256(data) for name, data in sorted(payload.items())}))
    runtime = runtime.replace("@STEM@", verify.source_stem(profile))
    recipe = replace_once(recipe, "\n_verify() {\n", runtime + "\n_verify() {\n")
    verify.require("$startdir" not in recipe, "unadapted startdir reference")
    # build/check/package retain every original instruction, with only kit paths moved.
    verify.require(original.count("build() {\n") == recipe.count("build() {\n") == 1,
                   "reviewed build function anchor changed or duplicated")
    verify.require(recipe[recipe.index("build() {\n"):] ==
                   original[original.index("build() {\n"):].replace("$startdir", "$srcdir/cua-profile-kit"),
                   "build/check/package changed during adaptation")
    return ("# Separately reviewed download wrapper; original kit and source identity are unchanged.\n"
            "# Normal reruns need a fresh build directory; makepkg -e reuses verified extracted trees.\n"
            + recipe).encode()


def generate(archive, expected_sha, download_url, output):
    verify.require(isinstance(download_url, str) and re.fullmatch(
        r"https://github\.com/trycua/cua/releases/download/[A-Za-z0-9][A-Za-z0-9._-]*/" + re.escape(archive.name),
        download_url), "requires exact trycua/cua release URL, safe tag, and archive filename")
    payload, profile, provenance = reviewed_kit(archive, expected_sha)
    recipe = adapt_recipe(payload, profile, provenance, archive.name, expected_sha, download_url)
    # Never truncate an existing recipe, including a symlink target.
    with output.open("xb") as destination:
        destination.write(recipe)
    return recipe


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--sha256", required=True, help="independently reviewed outer archive SHA-256")
    parser.add_argument("--url", required=True, help="exact future GitHub release asset URL")
    parser.add_argument("--output", required=True, type=Path, help="new downstream PKGBUILD file")
    args = parser.parse_args()
    try:
        generate(args.archive, args.sha256, args.url, args.output)
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
