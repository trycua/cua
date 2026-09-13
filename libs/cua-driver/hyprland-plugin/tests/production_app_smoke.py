"""Bounded native Calc/Inkscape keyboard smoke on a disposable Hyprland VM.

Setup must load and enable the production plugin before this runner starts.
Only new synthetic documents inside --evidence are opened. Missing semantic
grounding produces inspection_only, never a pass. This is not isolation,
concurrency, pointer-operation, or full desktop-matrix certification.
"""
import argparse
import ctypes
import hashlib
import io
import json
import math
import mmap
import os
from pathlib import Path
import re
import stat
import subprocess
import time
import xml.etree.ElementTree as ET
import zipfile

from production_mcp import DirectMCP


PACKAGES = {'libreoffice-fresh': '26.2.5-3', 'inkscape': '1.4.4-6'}
APP_PROFILES = {'calc-inkscape': ('calc', 'inkscape'), 'inkscape-only': ('inkscape',)}
EXECUTABLES = {'calc': Path('/usr/lib/libreoffice/program/soffice.bin'),
               'inkscape': Path('/usr/bin/inkscape')}
NS = {'office': 'urn:oasis:names:tc:opendocument:xmlns:office:1.0',
      'table': 'urn:oasis:names:tc:opendocument:xmlns:table:1.0',
      'text': 'urn:oasis:names:tc:opendocument:xmlns:text:1.0',
      'svg': 'http://www.w3.org/2000/svg'}
LIMITS = {'cursor_isolation': False, 'concurrency': False,
          'plugin_transport_attribution': False,
          'all_operations': False, 'full_desktop_matrix': False}


class GroundingUnavailable(RuntimeError):
    """The snapshots do not authorize the next deterministic key action."""


def read(command):
    return subprocess.check_output(command, text=True, timeout=10).strip()


def digest(path):
    return {'path': str(path.resolve(strict=True)),
            'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def save_json(directory, name, value):
    (directory / name).write_text(json.dumps(value, indent=2) + '\n')


def package_owner(path, package):
    assert path.resolve(strict=True) == path, 'noncanonical package executable'
    assert path.is_file() and path.stat().st_mode & 0o111, 'package executable is not executable'
    assert read(['pacman', '-Qqo', str(path)]) == package, 'noncanonical package owner'
    assert read(['pacman', '-Q', package]) == f'{package} {PACKAGES[package]}'
    return digest(path)


def profile_packages(name):
    assert isinstance(name, str) and name in APP_PROFILES, 'unknown app qualification profile'
    return {package: PACKAGES[package] for app in APP_PROFILES[name]
            for package in ('libreoffice-fresh' if app == 'calc' else app,)}


def kernel_file_identity(plugin):
    """Read the kernel mapping identity of the exact open candidate file.

    Btrfs can report a subvolume device in stat() but the filesystem device in
    proc maps. Compare two proc-map identities, not different device namespaces.
    This private, non-executable reference mapping never loads code or writes
    the file. Keep the descriptor and mapping alive until the checks finish.
    """
    def fingerprint(info):
        return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)

    descriptor = os.open(plugin, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as stream:
        opened = os.fstat(stream.fileno())
        assert stat.S_ISREG(opened.st_mode) and opened.st_size > 0, 'candidate must be a nonempty regular file'
        # ACCESS_COPY is writable only to let ctypes expose the mapping address;
        # no byte is written, and the file descriptor itself is read-only.
        with mmap.mmap(stream.fileno(), 1, access=mmap.ACCESS_COPY) as reference:
            address = ctypes.addressof(ctypes.c_char.from_buffer(reference))
            matches = []
            for line in Path('/proc/self/maps').read_text().splitlines():
                fields = line.split(maxsplit=5)
                if len(fields) != 6:
                    continue
                start, end = (int(value, 16) for value in fields[0].split('-'))
                if not start <= address < end:
                    continue
                assert fields[5] == str(plugin), 'candidate reference mapping path differs or was deleted'
                assert int(fields[2], 16) == 0, 'candidate reference mapping has wrong offset'
                major, minor = (int(value, 16) for value in fields[3].split(':'))
                identity = (major, minor, int(fields[4]))
                assert identity[2] == opened.st_ino, 'candidate reference mapping inode differs'
                matches.append(identity)
            assert len(matches) == 1, 'cannot identify exact candidate reference mapping'
            assert fingerprint(os.fstat(stream.fileno())) == fingerprint(opened), 'open candidate changed'
            assert fingerprint(plugin.stat(follow_symlinks=False)) == fingerprint(opened), \
                'candidate path was replaced or changed'
            return matches[0]


def mapped_plugin(maps, plugin):
    """Require current path AND kernel device/inode; reject replaced/deleted DSOs."""
    expected = kernel_file_identity(plugin)
    matches = []
    for line in maps.splitlines():
        fields = line.split(maxsplit=5)
        if len(fields) != 6:
            continue
        path = fields[5]
        if Path(path.removesuffix(' (deleted)')).name != plugin.name:
            continue
        assert path == str(plugin), 'different or deleted plugin mapped in compositor'
        major, minor = (int(part, 16) for part in fields[3].split(':'))
        assert (major, minor, int(fields[4])) == expected, 'mapped plugin identity differs'
        matches.append(line)
    assert matches, 'declared plugin is not mapped in the active Hyprland process'
    return matches


def require_enabled_plugin(option):
    # The pinned Hyprland 0.56.2 getoption reply for a V2 CBoolValue uses
    # "bool": true, not the legacy integer config representation.
    assert isinstance(option, dict) and option.get('option') == 'plugin:cua:enabled' \
        and option.get('bool') is True and 'int' not in option, 'setup has not enabled the plugin'


def checkout_identity(source, expected_sha, label):
    source = source.resolve(strict=True)
    assert Path(read(['git', '-C', str(source), 'rev-parse', '--show-toplevel'])).resolve() == source
    sha = read(['git', '-C', str(source), 'rev-parse', 'HEAD'])
    assert re.fullmatch(r'[0-9a-f]{40}', expected_sha) and sha == expected_sha, f'{label} SHA mismatch'
    dirty = read(['git', '-C', str(source), 'status', '--porcelain'])
    assert not dirty, f'{label} requires a clean checkout'
    return {'source': str(source), 'source_sha': sha, 'dirty': dirty,
            'branch': read(['git', '-C', str(source), 'branch', '--show-current'])}


def source_identities(args):
    product = checkout_identity(args.source, args.source_sha, 'source')
    harness_source = getattr(args, 'harness_source', None)
    harness_sha = getattr(args, 'harness_sha', None)
    assert (harness_source is None) == (harness_sha is None), 'harness source and SHA must be supplied together'
    harness = (checkout_identity(harness_source, harness_sha, 'harness')
               if harness_source is not None else dict(product))
    assert Path(__file__).resolve().is_relative_to(Path(harness['source'])), 'runner is outside declared harness source'
    return product, harness


def add_provenance_arguments(parser):
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--harness-source', type=Path)
    parser.add_argument('--harness-sha')
    parser.add_argument('--artifact-role', choices=('diagnostic', 'production'))
    parser.add_argument('--kit-manifest', type=Path)
    parser.add_argument('--profile-manifest', type=Path)
    parser.add_argument('--build-provenance', type=Path)


def artifact_identity(args, product, plugin, app_profile):
    """Bind production metadata to the module; diagnostics never borrow its claim."""
    role = getattr(args, 'artifact_role', None)
    paths = [getattr(args, name, None) for name in ('kit_manifest', 'profile_manifest', 'build_provenance')]
    assert all(path is not None for path in paths) or all(path is None for path in paths), \
        'kit, profile and build provenance must be supplied together'
    if app_profile == 'inkscape-only':
        assert role in ('diagnostic', 'production'), 'inkscape-only requires an explicit artifact role'
    if role == 'diagnostic':
        assert getattr(args, 'trace_socket', None), 'diagnostic proof requires a trace socket'
        assert not any(paths), 'diagnostic module cannot claim the trace-disabled production kit'
    if role == 'production' or any(paths):
        assert role == 'production' and all(paths), 'production identity requires all package provenance manifests'
        assert not getattr(args, 'trace_socket', None), 'production package proof cannot use diagnostic trace'
        kit_path, profile_path, build_path = paths
        kit, profile, build = [json.loads(path.read_text()) for path in paths]
        assert kit['schema'] == 1 and re.fullmatch(r'[0-9a-f]{40}', kit['tooling_revision'])
        assert kit['profile_sha256'] == digest(profile_path)['sha256'], 'kit profile digest mismatch'
        assert kit['source']['revision'] == product['source_sha'], 'kit product source mismatch'
        assert profile['source'] == kit['source'], 'profile product source mismatch'
        assert build['source']['source_revision'] == product['source_sha'], 'build product source mismatch'
        assert build['source']['driver_version'] == kit['source']['driver_version'], 'build Driver version mismatch'
        assert build['kit'] == kit and build['profile'] == profile, 'build kit/profile provenance mismatch'
        assert kit['cmake_options'] == {'CUA_HYPRLAND_INPUT': 'ON', 'CUA_HYPRLAND_TEST_INPUT': 'OFF',
                                        'CUA_HYPRLAND_INPUT_TRACE': 'OFF'}, 'not a production kit configuration'
        assert build['module_sha256'] == digest(plugin)['sha256'], 'build module digest mismatch'
        return {'role': role, 'kit': digest(kit_path), 'profile': digest(profile_path),
                'build': digest(build_path), 'profile_id': profile['profile_id'],
                'tooling_revision': kit['tooling_revision'], 'module_sha256': build['module_sha256']}
    return {'role': role or 'unspecified'}


def provenance(args, app_profile=None):
    app_profile = app_profile if app_profile is not None else getattr(args, 'app_profile', 'calc-inkscape')
    packages = profile_packages(app_profile)
    product, harness = source_identities(args)
    source = Path(product['source'])
    assert dict(line.split() for line in read(['pacman', '-Q', *packages]).splitlines()) == packages
    app_files = {app: package_owner(path,
                                   'libreoffice-fresh' if app == 'calc' else app)
                 for app, path in EXECUTABLES.items() if app in APP_PROFILES[app_profile]}
    if 'calc' in APP_PROFILES[app_profile]:
        launcher = Path('/usr/bin/libreoffice').resolve(strict=True)
        app_files['calc_launcher'] = package_owner(launcher, 'libreoffice-fresh')
    signature = os.environ.get('HYPRLAND_INSTANCE_SIGNATURE')
    assert signature, 'missing active Hyprland instance identity'
    instances = json.loads(read(['hyprctl', '-j', 'instances']))
    matches = [row for row in instances if row.get('instance') == signature]
    assert len(matches) == 1, 'cannot identify the active Hyprland instance'
    pid = matches[0]['pid']
    assert Path(f'/proc/{pid}/exe').resolve(strict=True).name == 'Hyprland'
    plugin = args.plugin.resolve(strict=True)
    maps = mapped_plugin(Path(f'/proc/{pid}/maps').read_text(), plugin)
    artifact = artifact_identity(args, product, plugin, app_profile)
    plugins = read(['hyprctl', 'plugin', 'list'])
    assert 'cua-hyprland-plugin' in plugins, 'plugin is mapped but not registered'
    enabled = json.loads(read(['hyprctl', '-j', 'getoption', 'plugin:cua:enabled']))
    save_json(args.evidence, 'plugin-enabled-option.json', enabled)
    require_enabled_plugin(enabled)
    files = {'driver': args.driver, 'plugin': plugin,
             **{name: Path(__file__).with_name(name) for name in
                ('production_app_smoke.py', 'production_mcp.py', 'driver_input_live.py')}}
    return {**product, 'harness': harness, 'artifact': artifact,
            'source_versions': {
                'driver': re.search(r'(?m)^version = "([^"]+)"',
                                    (source / 'libs/cua-driver/rust/Cargo.toml').read_text())[1],
                'plugin': re.search(r'project\(cua_hyprland_plugin VERSION ([\d.]+)',
                                    (source / 'libs/cua-driver/hyprland-plugin/CMakeLists.txt').read_text())[1]},
            'driver_version': read([str(args.driver), '--version']),
            'app_profile': app_profile, 'packages': packages, 'app_files': app_files,
            'files': {name: digest(path) for name, path in files.items()},
            'hyprland_pid': pid, 'hyprland_version': read(['hyprctl', 'version']),
            'loaded_plugins': plugins, 'plugin_maps': maps, 'plugin_enabled': enabled}


def create_calc_document(directory):
    ods = directory / 'cua-smoke-calc.ods'
    content = f'''<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="{NS['office']}" xmlns:table="{NS['table']}"
 xmlns:text="{NS['text']}" office:version="1.3"><office:body><office:spreadsheet>
<table:table table:name="Smoke"><table:table-row><table:table-cell/></table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>'''
    manifest = '''<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
<manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>
<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>'''
    with zipfile.ZipFile(ods, 'x') as archive:
        archive.writestr('mimetype', 'application/vnd.oasis.opendocument.spreadsheet',
                         compress_type=zipfile.ZIP_STORED)
        archive.writestr('content.xml', content)
        archive.writestr('META-INF/manifest.xml', manifest)
    return ods


def create_inkscape_document(directory):
    svg = directory / 'cua-smoke-inkscape.svg'
    with svg.open('x') as stream:
        stream.write('''<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300" viewBox="0 0 400 300">
<rect id="smoke-rectangle" x="40" y="60" width="80" height="50" fill="#336699"/>
</svg>''')
    return svg


def create_documents(directory, app_profile='calc-inkscape'):
    """Minimal native formats, with one blank A1 and one uniquely named rectangle."""
    profile_packages(app_profile)
    creators = {'calc': create_calc_document, 'inkscape': create_inkscape_document}
    return {app: creators[app](directory) for app in APP_PROFILES[app_profile]}


def verify_calc(before, after):
    assert before != after, 'Calc did not save a changed document'
    with zipfile.ZipFile(io.BytesIO(after)) as archive:
        root = ET.fromstring(archive.read('content.xml'))
    table = root.find('.//table:table', NS)
    assert table is not None and table.get(f"{{{NS['table']}}}name") == 'Smoke'
    row = table.find('table:table-row', NS)
    assert row is not None
    cell = row.find('table:table-cell', NS)
    assert cell is not None
    assert cell.get(f"{{{NS['office']}}}value-type") == 'string'
    assert ''.join(cell.itertext()) == 'a', 'saved A1 does not contain the expected letter'
    assert cell.get(f"{{{NS['table']}}}formula") is None
    return {'cell': 'A1', 'text': 'a', 'verified': True}


def rectangle(content):
    root = ET.fromstring(content)
    assert root.get('viewBox') == '0 0 400 300', 'coordinate system changed'
    nodes = root.findall('svg:rect', NS)
    assert len(nodes) == 1 and nodes[0].get('id') == 'smoke-rectangle'
    node = nodes[0]
    x, y = float(node.get('x', '0')), float(node.get('y', '0'))
    if node.get('transform'):
        match = re.fullmatch(r'translate\(\s*([-+\d.eE]+)[,\s]+([-+\d.eE]+)\s*\)', node.get('transform'))
        assert match, 'unsupported saved transform; geometry remains unproven'
        x += float(match[1])
        y += float(match[2])
    assert all(math.isfinite(value) for value in (x, y))
    return node, (x, y)


def verify_inkscape(before, after):
    assert before != after, 'Inkscape did not save a changed document'
    original, start = rectangle(before)
    saved, end = rectangle(after)
    assert all(float(original.get(key)) == float(saved.get(key)) for key in ('width', 'height'))
    # Default unmodified Inkscape preferences: one Right moves two SVG pixels.
    assert math.isclose(end[0] - start[0], 2, abs_tol=1e-6)
    assert math.isclose(end[1] - start[1], 0, abs_tol=1e-6)
    return {'rectangle': 'smoke-rectangle', 'translation': [2, 0], 'verified': True}


def content(result):
    assert not result.get('isError'), result
    return result['structuredContent']


def check_delivery(result):
    value = content(result)
    assert value.get('route') == 'synthetic_events', 'input did not use the synthetic-events route'
    assert value.get('effect') in ('confirmed', 'unverifiable'), 'partial/refused input cannot pass'
    delivery = value.get('delivery')
    assert delivery is None or delivery.get('mode') == 'background', 'unknown/foreground delivery cannot pass'
    return value


def rows(snapshot):
    elements = snapshot.get('elements', [])
    if not elements:
        raise GroundingUnavailable('snapshot has no semantic elements; inspect retained screenshot')
    if any(str(row.get('role', '')).lower() in ('dialog', 'alert', 'alert dialog') for row in elements):
        raise GroundingUnavailable('unexpected dialog; no dismissal keys were sent')
    return elements


def calc_formula_selection(snapshot, elements, expected='A1'):
    """Recognize the pinned GTK3 Calc name field, not an arbitrary matching label.

    Calc's non-actionable Formula Tool Bar appears only in tree_markdown.
    Cross-check that section's indexed name-field panel against structured
    rows; the panel's parent_index cannot identify an omitted toolbar.
    """
    lines = snapshot.get('tree_markdown', '').splitlines()
    headers = [index for index, line in enumerate(lines)
               if line.strip() == '- tool bar = "Formula Tool Bar"']
    if len(headers) != 1:
        return False
    start = headers[0]
    indent = len(lines[start]) - len(lines[start].lstrip())
    section = []
    for line in lines[start + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            break
        section.append(line.strip())
    candidates = [row for row in elements if row.get('role') == 'text'
                  and row.get('label') == expected and row.get('enabled') is True]
    if len(candidates) != 1:
        return False
    field = candidates[0]
    panels = [row for row in elements if row.get('element_index') == field.get('parent_index')
              and row.get('role') == 'panel' and row.get('enabled') is True]
    if len(panels) != 1:
        return False
    panel = panels[0]
    field_line = f'- [{field.get("element_index")}] text "{expected}" '
    panel_line = f'- [{panel.get("element_index")}] panel "" '
    return (sum(line.startswith(field_line) for line in section) == 1
            and sum(line.startswith(panel_line) for line in section) == 1
            and any(row.get('role') == 'combo box' and row.get('enabled') is True
                    and row.get('parent_index') == panel.get('element_index') for row in elements)
            and any(row.get('role') == 'table' and row.get('label') == 'Sheet Smoke'
                    for row in elements))


def inkscape_selection_command(snapshot, elements):
    """Ground Ctrl+A in the pinned document UI, which exposes no canvas role.

    The non-actionable selection status exists only in tree_markdown. Require
    the exact initial status, the fixture's object row, and Edit > Select All
    in both projections; a label mentioning a canvas is not evidence.
    """
    menus = [row for row in elements if row.get('role') == 'menu'
             and row.get('label') == 'Edit' and row.get('enabled') is True]
    objects = [row for row in elements if row.get('role') == 'table cell'
               and row.get('label') == 'smoke-rectangle' and row.get('enabled') is True]
    if len(menus) != 1 or len(objects) != 1:
        return False
    commands = [row for row in elements if row.get('role') == 'menu item'
                and row.get('label') == 'Select All' and row.get('enabled') is True
                and row.get('parent_index') == menus[0].get('element_index')]
    if len(commands) != 1:
        return False
    lines = [line.strip() for line in snapshot.get('tree_markdown', '').splitlines()]
    status = ('- label = "No objects selected. Click, Shift+click, Alt+scroll mouse '
              'on top of objects, or drag around objects to select."')
    return lines.count(status) == 1 and all(
        sum(line.startswith(f'- [{row.get("element_index")}] {row["role"]} "{row["label"]}" ')
            for line in lines) == 1
        for row in (menus[0], objects[0], commands[0]))


def inkscape_selected_rectangle(snapshot, elements):
    """Cross-check the pinned root rectangle's status and selection geometry.

    Inkscape omits its non-actionable status from structured elements. The
    actual status says "Rectangle  in root", not "1 object selected". Require
    that exact status plus the named object and the four indexed geometry
    controls in both projections; an arbitrary selection label cannot pass.
    """
    lines = [line.strip() for line in snapshot.get('tree_markdown', '').splitlines()]
    status = ('- label = "Rectangle  in root. Click selection again to toggle '
              'scale/rotation handles."')
    if lines.count(status) != 1 or any('No objects selected.' in line for line in lines):
        return False
    objects = [row for row in elements if row.get('role') == 'table cell'
               and row.get('label') == 'smoke-rectangle' and row.get('enabled') is True]
    if len(objects) != 1 or sum(line.startswith(
            f'- [{objects[0].get("element_index")}] table cell "smoke-rectangle" ')
            for line in lines) != 1:
        return False
    for axis, value in (('X', 40), ('Y', 60), ('W', 80), ('H', 50)):
        controls = [row for row in elements if row.get('role') == 'spin button'
                    and row.get('label') == f'{value:.3f}'
                    and row.get('value') == f'{value:.1f}' and row.get('enabled') is True]
        matches = []
        for row in controls:
            prefix = (f'- [{row.get("element_index")}] spin button "{value:.3f}" '
                      f'value="{value:.1f}" ')
            matches.extend(index for index, line in enumerate(lines) if index > 0
                           and line.startswith(prefix) and lines[index - 1] == f'- label = "{axis}:"')
        if len(matches) != 1:
            return False
    return True


def ground(snapshot, app, stage):
    elements = rows(snapshot)
    if app == 'calc' and stage == 'insert':
        selected = any(row.get('selected') is True and row.get('label') == 'A1'
                       and row.get('role') == 'table cell' for row in elements)
        named = any(row.get('label') == 'Name Box' and row.get('value') == 'A1' for row in elements)
        if not (selected or named or calc_formula_selection(snapshot, elements)):
            raise GroundingUnavailable('cannot prove initial Calc selection A1 from the snapshot')
    if app == 'inkscape':
        if stage == 'select':
            if not inkscape_selection_command(snapshot, elements):
                raise GroundingUnavailable('cannot ground Inkscape document selection shortcut')
        elif stage == 'move':
            if not inkscape_selected_rectangle(snapshot, elements):
                raise GroundingUnavailable('cannot prove the single rectangle is selected before Right')


def snapshot(mcp, target, filename, app, stage):
    result = content(mcp.tool('get_window_state', target))
    assert filename in result.get('window_title', ''), 'snapshot is not the synthetic document'
    windows = content(mcp.tool('list_windows', {}))['windows']
    owned = [row for row in windows if row.get('pid') == target['pid']]
    if len(owned) != 1 or owned[0]['window_id'] != target['window_id']:
        raise GroundingUnavailable('extra app window or dialog; inspect fresh snapshot and window list')
    ground(result, app, stage)
    return result


def input_step(mcp, target, filename, app, stage, tool, arguments):
    snapshot(mcp, target, filename, app, stage)
    try:
        response = mcp.tool(tool, {**target, **arguments, 'delivery_mode': 'background'})
    except BaseException:
        # DirectMCP forbids all RPC after an unknown result, including a snapshot.
        # A best-effort compositor observation retains state without replaying input.
        try:
            save_json(mcp.directory, 'unknown-outcome-windows.json',
                      json.loads(read(['hyprctl', '-j', 'clients'])))
        except Exception:
            pass
        raise
    try:
        snapshot(mcp, target, filename, app, 'after')
    finally:
        # A dialog cannot downgrade a partial/unknown delivery to inspection_only.
        check_delivery(response)


def discover(mcp, app, document, old_pids):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        windows = content(mcp.tool('list_windows', {}))['windows']
        matches = [row for row in windows if document.name in row.get('title', '')
                   and row.get('pid') not in old_pids]
        if matches:
            assert len(matches) == 1, 'ambiguous launched document window'
            window = matches[0]
            pid = window['pid']
            executable = Path(f'/proc/{pid}/exe').resolve(strict=True)
            assert executable == EXECUTABLES[app].resolve(strict=True), 'noncanonical running executable'
            assert str(document).encode() in Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0'), \
                'running process is not bound to the new document'
            clients = json.loads(read(['hyprctl', '-j', 'clients']))
            native = [row for row in clients if row.get('pid') == pid]
            assert len(native) == 1 and native[0].get('xwayland') is False, 'not one native Wayland app window'
            maps = Path(f'/proc/{pid}/maps').read_text()
            assert '/libgtk-3.so' in maps, 'running app did not load GTK3'
            if app == 'calc':
                assert '/libvclplug_gtk3lo.so' in maps, 'Calc did not load its GTK3 backend'
            return {key: window[key] for key in ('pid', 'window_id')}, {
                'window': window, 'hyprland_window': native[0], 'executable': digest(executable),
                'gtk3_maps': [line for line in maps.splitlines()
                              if '/libgtk-3.so' in line or '/libvclplug_gtk3lo.so' in line]}
        time.sleep(0.2)
    # A startup dialog may replace the document title. Capture only new,
    # canonical app processes; never send recovery/dismissal keys.
    for window in windows:
        pid = window.get('pid')
        if pid in old_pids or type(pid) is not int:
            continue
        try:
            executable = Path(f'/proc/{pid}/exe').resolve(strict=True)
            if executable == EXECUTABLES[app].resolve(strict=True):
                mcp.tool('get_window_state', {key: window[key] for key in ('pid', 'window_id')})
        except OSError:
            continue
    raise GroundingUnavailable('launched document window not discovered; inspect launch and window evidence')


def launch_arguments(app, document, directory):
    launch = {'launch_path': str(EXECUTABLES[app])}
    if app == 'calc':
        launch['launch_path'] = '/usr/bin/libreoffice'
        launch['additional_arguments'] = [f'-env:UserInstallation={(directory / "calc-profile").as_uri()}',
                                           '--norestore', '--nologo', '--calc', str(document)]
    else:
        # Inkscape 1.4.4 accepts positional documents, not --new-instance.
        # discover() still requires a new PID, exact executable and document.
        launch['additional_arguments'] = [str(document)]
    return launch


def require_background_target(target, directory):
    """Reject bad fixture setup before input; the plugin remains authoritative.

    Launching a native app can make it foreground. An external test fixture
    must own primary interaction before this standalone smoke sends keys.
    This read-only check does not move focus or prove pointer isolation.
    """
    active = json.loads(read(['hyprctl', '-j', 'activewindow']))
    save_json(directory, 'primary-window-before-input.json', active)
    pid = active.get('pid') if isinstance(active, dict) else None
    if type(pid) is not int or pid <= 0 or pid == target['pid']:
        raise GroundingUnavailable('a separate foreground fixture must be active before background input')


def run_app(mcp, app, document, directory):
    old_pids = {int(path.name) for path in Path('/proc').iterdir() if path.name.isdigit()}
    before = document.read_bytes()
    (directory / ('before' + document.suffix)).write_bytes(before)
    content(mcp.tool('launch_app', launch_arguments(app, document, directory)))
    target, identity = discover(mcp, app, document, old_pids)
    save_json(directory, 'target.json', identity)
    try:
        require_background_target(target, directory)
        if app == 'calc':
            steps = [('insert', 'press_key', {'key': 'a'}),
                     ('commit', 'press_key', {'key': 'Return'}),
                     ('save', 'hotkey', {'keys': ['ctrl', 's']})]
        else:
            steps = [('select', 'hotkey', {'keys': ['ctrl', 'a']}),
                     ('move', 'press_key', {'key': 'Right'}),
                     ('save', 'hotkey', {'keys': ['ctrl', 's']})]
        for stage, tool, arguments in steps:
            input_step(mcp, target, document.name, app, stage, tool, arguments)
        oracle = verify_calc if app == 'calc' else verify_inkscape
        # Read-only bounded save completion wait: never replay Ctrl+S.
        deadline = time.monotonic() + 10
        while True:
            try:
                result = oracle(before, document.read_bytes())
                break
            except (AssertionError, ET.ParseError, zipfile.BadZipFile, FileNotFoundError):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)
        snapshot(mcp, target, document.name, app, 'saved')
        return {'result': 'passed', 'target': target, 'saved_document': result}
    finally:
        if document.exists():
            (directory / ('after' + document.suffix)).write_bytes(document.read_bytes())


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence = args.evidence.resolve()
    args.driver = args.driver.resolve(strict=True)
    args.evidence.mkdir(parents=True, exist_ok=False)
    result = {'result': 'failed', 'apps': {}, 'limitations': LIMITS,
              'cleanup': 'App windows are retained for inspection; no app processes are closed.'}
    previous_env = {key: os.environ.get(key) for key in ('GDK_BACKEND', 'SAL_USE_VCLPLUGIN', 'XDG_CONFIG_HOME')}
    try:
        save_json(args.evidence, 'requested-inputs.json', {
            'source': str(args.source), 'source_sha': args.source_sha,
            'driver': digest(args.driver), 'plugin': digest(args.plugin)})
        save_json(args.evidence, 'provenance.json', provenance(args))
        app_profile = getattr(args, 'app_profile', 'calc-inkscape')
        result['app_profile'] = app_profile
        documents = create_documents(args.evidence, app_profile)
        # Inherited by the ordinary Driver launcher; preferences are run-local.
        os.environ.update(GDK_BACKEND='wayland', SAL_USE_VCLPLUGIN='gtk3',
                          XDG_CONFIG_HOME=str(args.evidence / 'config'))
        for app, document in documents.items():
            directory = args.evidence / app
            directory.mkdir()
            mcp = None
            try:
                mcp = DirectMCP(args.driver, directory,
                                {'mode': 'unrestricted', 'acknowledge_unrestricted': True})
                result['apps'][app] = run_app(mcp, app, document, directory)
            except GroundingUnavailable as error:
                result['apps'][app] = {'result': 'inspection_only', 'blocker': str(error)}
            except Exception as error:
                result['apps'][app] = {'result': 'failed', 'error': str(error)}
            finally:
                if mcp:
                    mcp.close()
        statuses = [row['result'] for row in result['apps'].values()]
        result['result'] = ('passed' if statuses == ['passed'] * len(APP_PROFILES[app_profile]) else
                            'failed' if 'failed' in statuses else 'inspection_only')
    except Exception as error:
        result['error'] = str(error)
    finally:
        for key, value in previous_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        save_json(args.evidence, 'result.json', result)
    print(json.dumps(result), flush=True)
    return 0 if result['result'] == 'passed' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('source', 'driver', 'plugin', 'evidence'):
        parser.add_argument('--' + name, type=Path, required=True)
    add_provenance_arguments(parser)
    parser.add_argument('--app-profile', choices=APP_PROFILES, default='calc-inkscape')
    raise SystemExit(run(parser.parse_args()))
