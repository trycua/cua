"""Explicit app-profile contract tests; no native input or qualification claims."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from production_app_smoke import (PACKAGES, artifact_identity, create_documents,
                                  digest, package_owner, profile_packages, source_identities)
from production_realapp_proof import (capacity_reservations, inkscape_client_identity,
                                     validate_plan, verify_output)
from production_realapp_proof_test import capacity_plan, plan


def inkscape_plan(capacity=False):
    candidate = capacity_plan() if capacity else plan()
    candidate['app_profile'] = 'inkscape-only'
    candidate['package_versions'] = {'inkscape': '1.4.4-6'}
    for index, spec in enumerate(candidate['agents']):
        spec.update(app='inkscape', document=f'/synthetic/lane-{index}.svg')
    candidate['outputs'] = [] if capacity else [
        {'agent': i, 'path': spec['document'], 'format': 'svg',
         'xpath': './/svg:rect[@id="smoke-rectangle"]',
         'namespaces': {'svg': 'http://www.w3.org/2000/svg'},
         'rect_translation': [[2, 2], [0, 0]]}
        for i, spec in enumerate(candidate['agents'])]
    return candidate


class AppProfileTests(unittest.TestCase):
    def test_profile_is_explicit_and_keeps_exact_package_gate(self):
        self.assertEqual(profile_packages('calc-inkscape'), PACKAGES)
        self.assertEqual(profile_packages('inkscape-only'), {'inkscape': '1.4.4-6'})
        for value in ('all', 'inkscape', '', None):
            with self.subTest(value=value), self.assertRaises(AssertionError):
                profile_packages(value)

    def test_single_app_smoke_does_not_create_calc_document(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            documents = create_documents(directory, 'inkscape-only')
            self.assertEqual(list(documents), ['inkscape'])
            self.assertEqual(list(directory.iterdir()), [documents['inkscape']])
            with self.assertRaises(FileExistsError):
                create_documents(directory, 'inkscape-only')

    def test_inkscape_executable_owner_and_version_are_both_exact(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary).resolve() / 'inkscape'
            executable.write_bytes(b'synthetic executable')
            executable.chmod(0o700)
            for replies in (['other-package'], ['inkscape', 'inkscape 1.4.4-7']):
                with patch('production_app_smoke.read', side_effect=replies), self.assertRaises(AssertionError):
                    package_owner(executable, 'inkscape')
            with patch('production_app_smoke.read', side_effect=['inkscape', 'inkscape 1.4.4-6']):
                self.assertEqual(package_owner(executable, 'inkscape'), digest(executable))

    def test_apps_and_capacity_need_explicit_profile_and_independent_clients(self):
        for capacity in (False, True):
            candidate = inkscape_plan(capacity)
            validate_plan(candidate)
            for mutate in (
                lambda p: p.pop('app_profile'),
                lambda p: p.update(app_profile='arbitrary'),
                lambda p: p['agents'][0].update(app='calc'),
                lambda p: p['agents'][1]['target'].update(pid=p['agents'][0]['target']['pid']),
                lambda p: p['agents'][1]['target'].update(window_id=p['agents'][0]['target']['window_id']),
                lambda p: p['agents'][0]['target'].update(window_id=None),
                lambda p: p['agents'][1].update(document=p['agents'][0]['document']),
                lambda p: p['agents'][0].update(document='relative.svg'),
            ):
                bad = copy.deepcopy(candidate)
                mutate(bad)
                with self.subTest(capacity=capacity, plan=bad), self.assertRaises(AssertionError):
                    validate_plan(bad)
        # The historical Calc/Inkscape profiles keep working unchanged.
        validate_plan(plan())
        validate_plan(capacity_plan())

    def test_two_app_lanes_need_distinct_owned_native_svg_oracles(self):
        for mutate in (
            lambda p: p.update(outputs=p['outputs'][:1]),
            lambda p: p['outputs'][1].update(path=p['outputs'][0]['path']),
            lambda p: p['outputs'][0].update(path='/synthetic/other.svg'),
            lambda p: p['outputs'][0].update(format='ods'),
            lambda p: p['outputs'][0].update(zip_member='content.xml'),
            lambda p: p['outputs'][0].pop('rect_translation'),
        ):
            candidate = inkscape_plan()
            mutate(candidate)
            with self.subTest(plan=candidate), self.assertRaises(AssertionError):
                validate_plan(candidate)

    def test_capacity_keeps_two_admissions_exact_third_refusal_and_serial_order(self):
        for mutate in (
            lambda p: p.update(agents=p['agents'][:2]),
            lambda p: p['phases'][2].update(expect={'kind': 'dispatched'}),
            lambda p: p['phases'][2]['expect'].update(reason='target_unavailable'),
            lambda p: p['phases'][2].update(agent=0),
            lambda p: p.update(require_overlap=True),
            lambda p: p.update(moving_primary=True),
        ):
            candidate = inkscape_plan(True)
            mutate(candidate)
            with self.subTest(plan=candidate), self.assertRaises(AssertionError):
                validate_plan(candidate)

    def test_capacity_owners_must_retain_both_reservations_through_third_refusal(self):
        status = {'state': 'input_v3_candidate', 'input': {'protocol': 3, 'test_only': False,
                  'transport_ready': True, 'lanes': [
                      {'lane': lane, 'reserved': True, 'lease_active': False, 'drag_active': False,
                       'held_keys': 0, 'held_button': 0, 'epoch': 50 + lane, 'desktop_generation': 4}
                      for lane in (0, 1)]}}
        previous = capacity_reservations(status, [1], {})
        both = capacity_reservations(status, [1, 2], previous)
        self.assertEqual(capacity_reservations(status, [1, 2], both), both)
        for lane in (0, 1):
            for change in ({'reserved': False}, {'epoch': 100}, {'desktop_generation': 5},
                           {'held_keys': 1}, {'held_button': 272}, {'lease_active': True}):
                bad = copy.deepcopy(status)
                bad['input']['lanes'][lane].update(change)
                with self.subTest(lane=lane, change=change), self.assertRaises(AssertionError):
                    capacity_reservations(bad, [1, 2], both)

    def test_saved_svg_must_change_exact_rectangle_without_resizing(self):
        with tempfile.TemporaryDirectory() as temporary:
            original = create_documents(Path(temporary), 'inkscape-only')['inkscape'].read_bytes()
        oracle = inkscape_plan()['outputs'][0]
        changed = original.replace(b'x="40"', b'x="42"')
        self.assertTrue(verify_output(original, changed, oracle)['verified'])
        for bad in (original, changed.replace(b'<svg ', b'<other ').replace(b'</svg>', b'</other>'),
                    changed.replace(b'width="80"', b'width="81"'),
                    changed.replace(b'x="42"', b'x="44"')):
            with self.assertRaises(AssertionError):
                verify_output(original, bad, oracle)

    def test_prelaunched_process_must_own_exact_native_document(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            document = create_documents(root, 'inkscape-only')['inkscape']
            process = root / '20'
            process.mkdir()
            (process / 'cmdline').write_bytes(b'/usr/bin/inkscape\0' + str(document).encode() + b'\0')
            spec = {'target': {'pid': 20, 'window_id': 200}, 'document': str(document)}
            window = {'pid': 20, 'address': '0xc8', 'xwayland': False, 'title': document.name}
            with patch('production_realapp_proof.app_process_identity', return_value={'pid': 20}) as identity:
                result = inkscape_client_identity(spec, [window], root)
                identity.assert_called_once_with('inkscape', 20, root)
                self.assertEqual(result['document']['path'], str(document))
                for change in ({'pid': 21}, {'address': '0xc9'}, {'xwayland': True},
                               {'title': 'unrelated.svg'}):
                    with self.subTest(change=change), self.assertRaises(AssertionError):
                        inkscape_client_identity(spec, [{**window, **change}], root)
                with self.assertRaises(AssertionError):
                    inkscape_client_identity(spec, [window, window], root)
                (process / 'cmdline').write_bytes(b'/usr/bin/inkscape\0/unrelated.svg\0')
                with self.assertRaisesRegex(AssertionError, 'not bound'):
                    inkscape_client_identity(spec, [window], root)


class ProfileProvenanceTests(unittest.TestCase):
    def test_helper_clis_expose_separate_source_and_artifact_options(self):
        names = ('app_smoke', 'realapp_proof', 'active_lock_proof', 'active_primary_proof',
                 'cancel_proof', 'desktop_fault_proof', 'geometry_fault_proof', 'idle_reconnect_proof',
                 'lock_refusal_proof', 'primary_conflict_proof', 'session_fault_proof',
                 'target_lifetime_proof', 'policy_proof')
        for name in names:
            with self.subTest(helper=name):
                output = subprocess.check_output(
                    [sys.executable, str(Path(__file__).with_name(f'production_{name}.py')), '--help'],
                    text=True, timeout=10)
                for option in ('--source-sha', '--harness-source', '--harness-sha', '--artifact-role',
                               '--kit-manifest', '--profile-manifest', '--build-provenance'):
                    self.assertIn(option, output)

    def test_clean_exact_product_and_harness_checkouts_are_independent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            product, harness = root / 'product', root / 'harness'
            product.mkdir()
            harness.mkdir()
            revisions = {str(product): 'a' * 40, str(harness): 'b' * 40}
            def command(args):
                checkout = args[2]
                return {'--show-toplevel': checkout, 'HEAD': revisions[checkout],
                        '--porcelain': '', '--show-current': ''}[args[-1]]
            args = SimpleNamespace(source=product, source_sha='a' * 40,
                                   harness_source=harness, harness_sha='b' * 40)
            with patch('production_app_smoke.read', side_effect=command), \
                    patch('production_app_smoke.__file__', str(harness / 'tests/runner.py')):
                source, runner = source_identities(args)
                self.assertEqual(source['source_sha'], 'a' * 40)
                self.assertEqual(runner['source_sha'], 'b' * 40)
                self.assertEqual(runner['branch'], '')  # Detached stays detached.
                for change in ({'harness_sha': 'c' * 40}, {'harness_sha': None},
                               {'harness_source': product, 'harness_sha': 'a' * 40},
                               {'source_sha': 'c' * 40}):
                    with self.subTest(change=change), self.assertRaises(AssertionError):
                        source_identities(SimpleNamespace(**{**vars(args), **change}))
                for checkout in (product, harness):
                    def dirty(command_args):
                        return ' M changed.py' if command_args[2] == str(checkout) and command_args[-1] == '--porcelain' else command(command_args)
                    with patch('production_app_smoke.read', side_effect=dirty), self.assertRaisesRegex(AssertionError, 'clean checkout'):
                        source_identities(args)

    def test_production_manifest_trio_binds_source_profile_and_actual_module(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            plugin = root / 'plugin.so'
            plugin.write_bytes(b'synthetic module bytes')
            source = {'revision': 'a' * 40, 'driver_version': '0.24.0'}
            profile = {'profile_id': 'synthetic-profile', 'source': source}
            profile_path = root / 'PROFILE.json'
            profile_path.write_text(json.dumps(profile))
            kit = {'schema': 1, 'tooling_revision': 'b' * 40, 'source': source,
                   'profile_sha256': digest(profile_path)['sha256'],
                   'cmake_options': {'CUA_HYPRLAND_INPUT': 'ON', 'CUA_HYPRLAND_TEST_INPUT': 'OFF',
                                     'CUA_HYPRLAND_INPUT_TRACE': 'OFF'}}
            build = {'source': {'source_revision': source['revision'], 'driver_version': '0.24.0'},
                     'profile': profile, 'kit': kit, 'module_sha256': digest(plugin)['sha256']}
            kit_path, build_path = root / 'KIT-PROVENANCE.json', root / 'BUILD-PROVENANCE.json'
            kit_path.write_text(json.dumps(kit))
            build_path.write_text(json.dumps(build))
            args = SimpleNamespace(artifact_role='production', kit_manifest=kit_path,
                                   profile_manifest=profile_path, build_provenance=build_path)
            product = {'source_sha': source['revision']}
            self.assertEqual(artifact_identity(args, product, plugin, 'inkscape-only')['role'], 'production')
            for change in ({'artifact_role': 'diagnostic', 'trace_socket': root / 'trace.sock'},
                           {'build_provenance': None}, {'trace_socket': root / 'trace.sock'},
                           {'artifact_role': None}):
                with self.subTest(change=change), self.assertRaises(AssertionError):
                    artifact_identity(SimpleNamespace(**{**vars(args), **change}), product, plugin, 'inkscape-only')
            for change in ({'module_sha256': '0' * 64}, {'profile': {}}, {'kit': {}},
                           {'source': {'source_revision': 'c' * 40, 'driver_version': '0.24.0'}}):
                build_path.write_text(json.dumps({**build, **change}))
                with self.subTest(change=change), self.assertRaises(AssertionError):
                    artifact_identity(args, product, plugin, 'inkscape-only')
            build_path.write_text(json.dumps(build))
            bad_kit = copy.deepcopy(kit)
            bad_kit['cmake_options']['CUA_HYPRLAND_INPUT_TRACE'] = 'ON'
            kit_path.write_text(json.dumps(bad_kit))
            build_path.write_text(json.dumps({**build, 'kit': bad_kit}))
            with self.assertRaisesRegex(AssertionError, 'production kit configuration'):
                artifact_identity(args, product, plugin, 'inkscape-only')
            kit_path.write_text(json.dumps(kit))
            build_path.write_text(json.dumps(build))
            profile_path.write_text(json.dumps(profile) + '\n')
            with self.assertRaisesRegex(AssertionError, 'profile digest mismatch'):
                artifact_identity(args, product, plugin, 'inkscape-only')

    def test_diagnostic_identity_needs_trace_and_cannot_claim_production_kit(self):
        diagnostic = SimpleNamespace(artifact_role='diagnostic', trace_socket=Path('/trace.sock'))
        self.assertEqual(artifact_identity(diagnostic, {}, Path('/module'), 'inkscape-only'), {'role': 'diagnostic'})
        for args in (SimpleNamespace(), SimpleNamespace(artifact_role='diagnostic'),
                     SimpleNamespace(artifact_role='production')):
            with self.assertRaises(AssertionError):
                artifact_identity(args, {}, Path('/module'), 'inkscape-only')
        self.assertEqual(artifact_identity(SimpleNamespace(), {}, Path('/module'), 'calc-inkscape'),
                         {'role': 'unspecified'})


if __name__ == '__main__':
    unittest.main()
