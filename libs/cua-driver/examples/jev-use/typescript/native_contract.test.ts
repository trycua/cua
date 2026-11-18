// Role table and cua.jev_choice_request_v2 contract tests (RFC #4268).
// Mirrors python/tests/test_native_contract.py.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { chooseRequest, providerObservation, validateRequest } from './choose_action.js';
import {
  PLATFORMS,
  RAW_ROLES,
  ROLE_CLASSES,
  ROLE_CLASS_ACTION,
  ROLE_TABLES,
  normalizedRole,
  roleClass,
  type Platform,
} from './native_roles.js';

function fixture(name: string): any {
  return JSON.parse(readFileSync(new URL(`../fixtures/${name}`, import.meta.url), 'utf8'));
}

test('normalizedRole matches the Driver normalizer cases', () => {
  const cases: Record<string, string> = {
    AXCheckBox: 'checkbox',
    CheckBox: 'checkbox',
    'check box': 'checkbox',
    'push button': 'button',
    AXButton: 'button',
    'page tab': 'tab',
    TabItem: 'tab',
    AXTextField: 'textfield',
    'combo box': 'combobox',
  };
  for (const [raw, expected] of Object.entries(cases)) assert.equal(normalizedRole(raw), expected, raw);
});

test('each platform maps its raw roles and excludes the rest', () => {
  const mapped: Record<Platform, Record<string, string>> = {
    macos: {
      AXButton: 'button',
      AXCheckBox: 'checkbox',
      AXRadioButton: 'radio',
      AXPopUpButton: 'popup',
      AXMenuBarItem: 'menu_item',
      AXLink: 'link',
      AXTextField: 'text_input',
      AXSwitch: 'toggle',
    },
    windows: {
      Button: 'button',
      SplitButton: 'button',
      CheckBox: 'checkbox',
      RadioButton: 'radio',
      ComboBox: 'popup',
      MenuItem: 'menu_item',
      Hyperlink: 'link',
      Edit: 'text_input',
    },
    linux: {
      'push button': 'button',
      'toggle button': 'toggle',
      'check box': 'checkbox',
      'radio button': 'radio',
      'combo box': 'popup',
      'radio menu item': 'menu_item',
      link: 'link',
      entry: 'text_input',
      text: 'text_input',
    },
  };
  for (const platform of PLATFORMS) {
    for (const [raw, expected] of Object.entries(mapped[platform])) {
      assert.equal(roleClass(raw, platform), expected, `${platform} ${raw}`);
    }
  }
  const excluded: Record<Platform, string[]> = {
    macos: ['AXStaticText', 'AXSlider', 'AXWindow', 'AXMenuBar', ''],
    windows: ['Text', 'Slider', 'TabItem', 'Pane', 'Unknown'],
    linux: ['label', 'slider', 'page tab', 'frame'],
  };
  for (const platform of PLATFORMS) {
    for (const raw of excluded[platform]) assert.equal(roleClass(raw, platform), null, `${platform} ${raw}`);
  }
  assert.equal(roleClass(undefined, 'macos'), null);
});

test('role tables are keyed by the Driver normalization', () => {
  for (const platform of PLATFORMS) {
    for (const klass of ROLE_CLASSES) {
      for (const raw of RAW_ROLES[platform][klass]) {
        assert.equal(ROLE_TABLES[platform].get(normalizedRole(raw)), klass);
      }
    }
  }
  assert.deepEqual(new Set(Object.keys(ROLE_CLASS_ACTION)), new Set(ROLE_CLASSES));
});

test('v2 requests validate with sources and elements', () => {
  const validated = validateRequest(fixture('jev-choice-request-v2.json'));
  assert.equal(validated.schema, 'cua.jev_choice_request_v2');
  assert.equal(validated.snapshot_id, 's0000002a');
  assert.equal(validated.elements?.length, 3);
  assert.equal(validated.candidates[0].source, 'ax');
  assert.equal('source' in validated.candidates[1], false);
  const observation = providerObservation(validated);
  assert.deepEqual(observation.candidate_sources, { 'ax:button:increment': 'ax' });
  const v1 = providerObservation(validateRequest(fixture('jev-choice-request-v1.json')));
  assert.deepEqual(Object.keys(v1).sort(), ['capture_id', 'history', 'regions']);
});

test('v1 rejects v2 fields and v2 rejects malformed fields', () => {
  const v1Mutations: ((r: any) => void)[] = [
    (r) => (r.snapshot_id = 's1'),
    (r) => (r.elements = []),
    (r) => (r.candidates[0].source = 'visual'),
  ];
  for (const mutate of v1Mutations) {
    const request = fixture('jev-choice-request-v1.json');
    mutate(request);
    assert.throws(() => validateRequest(request));
  }
  const v2Mutations: Record<string, (r: any) => void> = {
    'unknown root key': (r) => (r.state = {}),
    'unknown source': (r) => (r.candidates[0].source = 'dom'),
    'reserved with source': (r) => (r.candidates[1].source = 'ax'),
    'candidate token': (r) => (r.candidates[0].element_token = 's1:3'),
    'element value': (r) => (r.elements[0].value = 'secret'),
    'unknown role class': (r) => (r.elements[0].role_class = 'slider'),
    'unknown state': (r) => (r.elements[0].state = 'hidden'),
    'too many elements': (r) => (r.elements = Array(22).fill(r.elements).flat()),
    'missing required': (r) => delete r.regions,
    'unknown schema': (r) => (r.schema = 'cua.jev_choice_request_v3'),
  };
  for (const [name, mutate] of Object.entries(v2Mutations)) {
    const request = fixture('jev-choice-request-v2.json');
    mutate(request);
    assert.throws(() => validateRequest(request), Error, name);
  }
  const minimal = fixture('jev-choice-request-v2.json');
  delete minimal.snapshot_id;
  delete minimal.elements;
  for (const candidate of minimal.candidates) delete candidate.source;
  assert.deepEqual(validateRequest(minimal).elements, []);
});

test('mock chooser answers v2 with the unchanged response schema', async () => {
  const response = await chooseRequest(fixture('jev-choice-request-v2.json'), { mock: true });
  assert.equal(response.schema, 'cua.jev_choice_v1');
  assert.equal(response.selected_id, 'ax:button:increment');
});
