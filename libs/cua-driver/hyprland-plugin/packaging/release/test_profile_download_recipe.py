"""Trust-boundary and shell contracts for the separate downstream download wrapper."""

import io
import shutil
import subprocess
import tarfile
import unittest

import profile_bundle as bundle
import profile_download_recipe as download
import profile_verify as verify
import test_profile_release as fixtures


class DownloadRecipeTest(unittest.TestCase):
    fixture_class = fixtures.ProfileTest

    def setUp(self):
        self.fixture = self.fixture_class()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        output, self.kit, self.provenance = self.fixture.generate()
        self.archive = next(output.glob("*.tar.gz"))
        self.checksum = verify.digest(self.archive)
        self.url = "https://github.com/trycua/cua/releases/download/profile-kit-v1/" + self.archive.name
        self.output = self.root / "PKGBUILD"
        self.payload = download.archive_payload(self.archive.read_bytes())
        self.srcdir = self.root / "makepkg src"
        self.srcdir.mkdir()
        self.startdir = self.root / "unrelated startdir"
        self.startdir.mkdir()

    def generate(self):
        return download.generate(self.archive, self.checksum, self.url, self.output)

    def rewrite(self, payload, *, sums=True):
        if sums:
            payload["SHA256SUMS"] = "".join(f"{verify.sha256(data)}  {name}\n" for name, data in sorted(payload.items())
                                             if name != "SHA256SUMS").encode()
        self.archive.write_bytes(bundle.deterministic_archive(payload))
        self.checksum = verify.digest(self.archive)

    def shell(self, command):
        if not shutil.which("bash") or not shutil.which("sha256sum"):
            self.skipTest("bash and sha256sum are required for generated recipe execution")
        script = 'source "$1"; SRCDEST="$2"; srcdir="$3"; startdir="$4"; ' + command
        return subprocess.run(["bash", "-c", script, "test", str(self.output), str(self.archive.parent),
                               str(self.srcdir), str(self.startdir)], capture_output=True, text=True)

    def extract(self):
        self.generate()
        result = self.shell("_verify_download extract")
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.srcdir / "cua-profile-kit", self.srcdir / self.fixture.stem

    def test_deterministic_export_and_unchanged_kit(self):
        original_archive = self.archive.read_bytes()
        first = self.generate()
        second = download.generate(self.archive, self.checksum, self.url, self.root / "PKGBUILD.second")
        self.assertEqual(first, second)
        self.assertEqual(self.archive.read_bytes(), original_archive)
        self.assertNotEqual(first, self.payload["PKGBUILD"])
        self.assertIn(b"Separately reviewed download wrapper", first)
        self.assertIn(f"source=('{self.url}')".encode(), first)
        self.assertIn(f"sha256sums=('{self.checksum}')".encode(), first)
        self.assertIn(b'noextract=("$_download_name")', first)
        self.assertIn(b'python3 -I - "$SRCDEST/$_download_name"', first)
        self.assertIn(b'"$SRCDEST/$_download_name" | sha256sum -c - || return 1', first)
        self.assertNotIn(b"$startdir", first)
        if shutil.which("bash"):
            subprocess.run(["bash", "-n", str(self.output)], check=True, capture_output=True)

    def test_url_scope_and_shell_injection_refused(self):
        base = self.url.rsplit("/", 2)[0]
        bad_urls = [self.url.replace("https:", "http:"), self.url.replace("trycua/cua", "other/cua"),
                    self.url.replace("github.com", "github.com.evil.invalid"), self.url + "?download=1",
                    self.url + "#fragment", self.url + "\n", self.url.replace(self.archive.name, "other.tar.gz")]
        bad_urls += [base + "/" + tag + "/" + self.archive.name
                     for tag in ("../escape", ".", "..", "%2e%2e", "$(touch injected)", "tag';false;'")]
        for url in bad_urls:
            with self.subTest(url=url), self.assertRaisesRegex(ValueError, "release URL"):
                download.generate(self.archive, self.checksum, url, self.output)
        self.assertFalse(self.output.exists())

    def test_wrong_hash_and_archive_tamper_refused(self):
        for checksum in ("f" * 64, "F" * 64, "abc", "a" * 64 + "\n"):
            with self.subTest(checksum=checksum), self.assertRaises(ValueError):
                download.generate(self.archive, checksum, self.url, self.output)
        self.archive.write_bytes(self.archive.read_bytes() + b"tamper")
        with self.assertRaisesRegex(ValueError, "outer archive checksum"):
            self.generate()
        self.assertFalse(self.output.exists())

    def test_nonregular_or_renamed_archive_and_existing_output_refused(self):
        linkdir = self.root / "links"
        linkdir.mkdir()
        link = linkdir / self.archive.name
        link.symlink_to(self.archive)
        with self.assertRaisesRegex(ValueError, "regular file"):
            download.generate(link, self.checksum, self.url, self.output)
        renamed = self.root / "renamed.tar.gz"
        renamed.write_bytes(self.archive.read_bytes())
        with self.assertRaisesRegex(ValueError, "filename"):
            download.generate(renamed, self.checksum, self.url.rsplit("/", 1)[0] + "/" + renamed.name, self.output)
        self.output.write_bytes(b"existing work")
        with self.assertRaises(FileExistsError):
            self.generate()
        self.assertEqual(self.output.read_bytes(), b"existing work")
        output_link = self.root / "output-link"
        output_link.symlink_to(self.output)
        with self.assertRaises(FileExistsError):
            download.generate(self.archive, self.checksum, self.url, output_link)
        self.assertEqual(self.output.read_bytes(), b"existing work")

    def test_outer_inventory_paths_duplicates_and_links_refused(self):
        for variant in ("symlink", "hardlink", "directory", "fifo", "duplicate", "traversal", "absolute", "backslash", "extra", "missing"):
            raw = io.BytesIO()
            with tarfile.open(fileobj=raw, mode="w:gz") as archive:
                for name, data in self.payload.items():
                    if variant == "missing" and name == "PROFILE.json":
                        continue
                    member = tarfile.TarInfo(name)
                    member.size = len(data)
                    archive.addfile(member, io.BytesIO(data))
                if variant != "missing":
                    name = {"duplicate": "PROFILE.json", "traversal": "../PROFILE.json", "absolute": "/PROFILE.json",
                            "backslash": "a\\PROFILE.json"}.get(variant, "extra")
                    member = tarfile.TarInfo(name)
                    member.type = {"symlink": tarfile.SYMTYPE, "hardlink": tarfile.LNKTYPE,
                                   "directory": tarfile.DIRTYPE, "fifo": tarfile.FIFOTYPE}.get(variant, tarfile.REGTYPE)
                    member.linkname = "PROFILE.json" if variant in {"symlink", "hardlink"} else ""
                    archive.addfile(member)
            self.archive.write_bytes(raw.getvalue())
            self.checksum = verify.digest(self.archive)
            with self.subTest(variant=variant), self.assertRaises(ValueError):
                self.generate()

    def test_internal_hashes_provenance_and_unreviewed_code_refused(self):
        for name in ("SHA256SUMS", "PROFILE.json", "KIT-PROVENANCE.json", "SOURCE-PROVENANCE.json",
                     "profile_verify.py", "PKGBUILD", self.fixture.stem + ".tar.gz"):
            candidate = dict(self.payload)
            candidate[name] += b"\n# tampered\n"
            self.rewrite(candidate, sums=name != "SHA256SUMS")
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.generate()
        candidate = dict(self.payload)
        candidate["PROFILE-USAGE.md"] += b"tampered"
        self.rewrite(candidate, sums=False)
        with self.assertRaisesRegex(ValueError, "SHA256SUMS"):
            self.generate()

    def test_source_inventory_validated_even_with_consistent_outer_hashes(self):
        files = {self.fixture.stem + "/" + name: data for name, data in self.fixture.files.items()}
        files[self.fixture.stem + "/extra"] = b"unexpected source"
        candidate = dict(self.payload)
        candidate[self.fixture.stem + ".tar.gz"] = bundle.deterministic_archive(files)
        profile = verify.read_json(candidate["PROFILE.json"])
        profile["source"]["archive_sha256"] = verify.sha256(candidate[self.fixture.stem + ".tar.gz"])
        candidate["PROFILE.json"] = verify.json_bytes(profile)
        provenance = verify.read_json(candidate["KIT-PROVENANCE.json"])
        provenance["source"] = profile["source"]
        provenance["profile_sha256"] = verify.sha256(candidate["PROFILE.json"])
        candidate["KIT-PROVENANCE.json"] = verify.json_bytes(provenance)
        candidate["PKGBUILD"] = verify.render_recipe(candidate["PROFILE-PKGBUILD.in"].decode(), profile, provenance)
        self.rewrite(candidate)
        with self.assertRaisesRegex(ValueError, "source archive inventory"):
            self.generate()

    def test_source_extended_metadata_refused_before_export(self):
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w:gz", format=tarfile.PAX_FORMAT) as archive:
            for name, data in self.fixture.files.items():
                member = tarfile.TarInfo(self.fixture.stem + "/" + name)
                member.size = len(data)
                member.pax_headers = {"comment": "unreviewed metadata"}
                archive.addfile(member, io.BytesIO(data))
        candidate = dict(self.payload)
        candidate[self.fixture.stem + ".tar.gz"] = raw.getvalue()
        profile = verify.read_json(candidate["PROFILE.json"])
        profile["source"]["archive_sha256"] = verify.sha256(raw.getvalue())
        candidate["PROFILE.json"] = verify.json_bytes(profile)
        provenance = verify.read_json(candidate["KIT-PROVENANCE.json"])
        provenance["source"] = profile["source"]
        provenance["profile_sha256"] = verify.sha256(candidate["PROFILE.json"])
        candidate["KIT-PROVENANCE.json"] = verify.json_bytes(provenance)
        candidate["PKGBUILD"] = verify.render_recipe(candidate["PROFILE-PKGBUILD.in"].decode(), profile, provenance)
        self.rewrite(candidate)
        with self.assertRaisesRegex(ValueError, "extended member"):
            self.generate()

    def test_provenance_bytes_must_match_inner_recipe_checksum(self):
        candidate = dict(self.payload)
        candidate["KIT-PROVENANCE.json"] += b"\n"
        self.rewrite(candidate)
        with self.assertRaisesRegex(ValueError, "canonical checksum"):
            self.generate()

    def test_prepare_rejects_tampered_outer_before_any_extraction(self):
        self.generate()
        self.archive.write_bytes(b"untrusted archive")
        result = self.shell("SKIPINTEG=1; prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("outer archive checksum", result.stderr)
        self.assertEqual(list(self.srcdir.iterdir()), [])

    def test_adaptation_refuses_missing_or_duplicated_anchors(self):
        for old, new in ((b"prepare() {\n  _verify\n}", b"prepare() {\n true\n}"),
                         (b"prepare() {\n  _verify\n}", b"prepare() {\n  _verify\n}\nprepare() {\n  _verify\n}"),
                         (b"$startdir/$name", b"$other/$name")):
            candidate = dict(self.payload)
            candidate["PKGBUILD"] = candidate["PKGBUILD"].replace(old, new)
            with self.subTest(old=old), self.assertRaisesRegex(ValueError, "anchor|references"):
                download.adapt_recipe(candidate, self.fixture.profile, self.provenance, self.archive.name, self.checksum, self.url)

    def test_prepare_extracts_exact_kit_and_source_with_distinct_srcdest(self):
        kit, source = self.extract()
        self.assertEqual({p.name: p.read_bytes() for p in kit.iterdir()}, self.payload)
        self.assertEqual(verify.verify_source(source, self.fixture.profile), self.fixture.manifest)
        result = self.shell('_verify_download check && [[ "$startdir" == "$4" ]]')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(list(self.startdir.iterdir()), [])
        result = self.shell("_verify_download extract")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("fresh kit and source", result.stderr)

    def test_wrapper_owned_check_ignores_pythonpath_import_shadowing(self):
        self.generate()
        shadow = self.root / "shadow imports"
        shadow.mkdir()
        (shadow / "hashlib.py").write_text("raise SystemExit('UNTRUSTED_IMPORT_EXECUTED')\n")
        result = self.shell('export PYTHONPATH="$4/../shadow imports"; _verify_download extract')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("UNTRUSTED_IMPORT_EXECUTED", result.stderr)

    def test_prepare_refuses_existing_symlink_destinations_before_writes(self):
        self.generate()
        for name in ("cua-profile-kit", self.fixture.stem):
            link = self.srcdir / name
            link.symlink_to(self.startdir, target_is_directory=True)
            result = self.shell("prepare")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("fresh kit and source", result.stderr)
            self.assertEqual(list(self.startdir.iterdir()), [])
            link.unlink()

    def test_every_phase_refuses_tampered_outer_kit_and_source(self):
        kit, source = self.extract()
        targets = [self.archive, kit / "PROFILE.json", kit / "profile_verify.py", kit / "KIT-PROVENANCE.json",
                   kit / (self.fixture.stem + ".tar.gz"), kit / "PKGBUILD", source / "src/plugin.cpp"]
        for target in targets:
            original = target.read_bytes()
            target.write_bytes(b"raise SystemExit('UNTRUSTED_CODE_EXECUTED')\n")
            for phase in ("_verify", "build", "check", "package"):
                with self.subTest(target=target.name, phase=phase):
                    result = self.shell(phase)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn("UNTRUSTED_CODE_EXECUTED", result.stderr)
                    self.assertIn("checksum", result.stderr)
            target.write_bytes(original)

    def test_extracted_kit_and_source_symlinks_and_extra_files_refused(self):
        kit, source = self.extract()
        for target in (kit / "profile_verify.py", source / "src/plugin.cpp"):
            original = target.read_bytes()
            reference = self.root / "reference"
            reference.write_bytes(original)
            target.unlink()
            target.symlink_to(reference)
            result = self.shell("_verify")
            self.assertNotEqual(result.returncode, 0)
            target.unlink()
            target.write_bytes(original)
        for directory in (kit, source):
            extra = directory / "extra"
            extra.write_bytes(b"extra")
            result = self.shell("_verify")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("inventory", result.stderr)
            extra.unlink()

    def test_build_check_package_are_preserved_and_ctest_failure_blocks_package(self):
        recipe = self.generate().decode()
        original = self.payload["PKGBUILD"].decode()
        self.assertEqual(recipe[recipe.index("build() {\n"):],
                         original[original.index("build() {\n"):].replace("$startdir", "$srcdir/cua-profile-kit"))
        for required in ("--no-tests=error", "-DBUILD_TESTING=ON", "-DCUA_HYPRLAND_INPUT=ON",
                         "-DCUA_HYPRLAND_TEST_INPUT=OFF", "-DCUA_HYPRLAND_INPUT_TRACE=OFF",
                         '_verify --build "$srcdir/build" --output "$srcdir/BUILD-PROVENANCE.json"',
                         'unset LD_PRELOAD FAKEROOTKEY FAKED_MODE'):
            self.assertIn(required, recipe)
        result = self.shell('_verify() { return 0; }; ctest() { return 17; }; '
                            'install() { echo UNEXPECTED_INSTALL >&2; return 0; }; package')
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("UNEXPECTED_INSTALL", result.stderr)


class Schema2DownloadRecipeTest(DownloadRecipeTest):
    fixture_class = fixtures.Schema2ProfileTest

    def test_outer_source_name_must_match_validated_profile(self):
        for name in (verify.STEM + ".tar.gz", "../" + self.fixture.stem + ".tar.gz",
                     self.fixture.stem + ".tar.gz/extra", self.fixture.stem.replace("0.26.0", "0.25.0") + ".tar.gz"):
            candidate = dict(self.payload)
            candidate[name] = candidate.pop(self.fixture.stem + ".tar.gz")
            self.rewrite(candidate)
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "inventory|path"):
                self.generate()
        self.assertFalse(self.output.exists())

    def test_wrapper_binds_candidate_source_and_version(self):
        kit, source = self.extract()
        recipe = self.output.read_text()
        self.assertIn("pkgver=" + self.fixture.driver_version, recipe)
        self.assertIn("stem = '" + self.fixture.stem + "'", recipe)
        self.assertNotIn(verify.STEM, recipe)
        self.assertEqual(source.name, self.fixture.stem)
        self.assertEqual((kit / (self.fixture.stem + ".tar.gz")).read_bytes(), self.fixture.archive.read_bytes())


if __name__ == "__main__":
    unittest.main()
