// Per-platform role table for native accessibility candidates (RFC #4268).
// Mirrors python/native_roles.py; see that module for the full rationale.
//
// Cua Driver reports raw platform roles (macOS AX, Windows UIA
// control_type_name, Linux AT-SPI role names) and does not normalize them, so
// jev-use owns this closed mapping. Every table is keyed by the output of
// normalizedRole, a port of Driver's private normalized_role in
// cua-driver-core/src/expectation.rs, so raw roles that Driver's verify_state
// treats as the same role can never map to different role classes here.

export type Platform = 'macos' | 'windows' | 'linux';
export type RoleClass =
  | 'button'
  | 'toggle'
  | 'checkbox'
  | 'radio'
  | 'popup'
  | 'menu_item'
  | 'link'
  | 'text_input';
export type ActionKind = 'press' | 'toggle' | 'select' | 'open_menu' | 'set_text' | 'visual_click';

export const PLATFORMS: readonly Platform[] = ['macos', 'windows', 'linux'];

export const ROLE_CLASSES: readonly RoleClass[] = [
  'button',
  'toggle',
  'checkbox',
  'radio',
  'popup',
  'menu_item',
  'link',
  'text_input',
];

export const ROLE_CLASS_ACTION: Readonly<Record<RoleClass, ActionKind>> = {
  button: 'press',
  toggle: 'toggle',
  checkbox: 'toggle',
  radio: 'select',
  popup: 'open_menu',
  menu_item: 'press',
  link: 'press',
  text_input: 'set_text',
};

export const ACTION_KINDS: ReadonlySet<ActionKind> = new Set<ActionKind>([
  'press',
  'toggle',
  'select',
  'open_menu',
  'set_text',
  'visual_click',
]);

export const RAW_ROLES: Readonly<Record<Platform, Readonly<Record<RoleClass, readonly string[]>>>> = {
  macos: {
    button: ['AXButton'],
    toggle: ['AXSwitch'],
    checkbox: ['AXCheckBox'],
    radio: ['AXRadioButton'],
    popup: ['AXPopUpButton', 'AXComboBox', 'AXMenuButton'],
    menu_item: ['AXMenuItem', 'AXMenuBarItem'],
    link: ['AXLink'],
    text_input: ['AXTextField', 'AXTextArea', 'AXSearchField', 'AXSecureTextField'],
  },
  // UIA control types; the WPF and WinUI3 harnesses report the same ones.
  windows: {
    button: ['Button', 'SplitButton'],
    toggle: [],
    checkbox: ['CheckBox'],
    radio: ['RadioButton'],
    popup: ['ComboBox'],
    menu_item: ['MenuItem'],
    link: ['Hyperlink'],
    text_input: ['Edit'],
  },
  linux: {
    button: ['push button', 'button'],
    toggle: ['toggle button', 'switch'],
    checkbox: ['check box'],
    radio: ['radio button'],
    popup: ['combo box'],
    menu_item: ['menu item', 'check menu item', 'radio menu item'],
    link: ['link'],
    text_input: ['entry', 'text', 'password text'],
  },
};

/** Port of Driver's normalized_role (cua-driver-core/src/expectation.rs). */
export function normalizedRole(role: string): string {
  let normalized = [...role]
    .filter((character) => /^[A-Za-z0-9]$/.test(character))
    .map((character) => character.toLowerCase())
    .join('');
  if (normalized.startsWith('ax')) normalized = normalized.slice(2);
  if (normalized === 'pushbutton') return 'button';
  if (normalized === 'pagetab' || normalized === 'tabitem') return 'tab';
  return normalized;
}

function normalizedTable(platform: Platform): ReadonlyMap<string, RoleClass> {
  const table = new Map<string, RoleClass>();
  for (const roleClass of ROLE_CLASSES) {
    for (const raw of RAW_ROLES[platform][roleClass]) {
      const key = normalizedRole(raw);
      const existing = table.get(key);
      if (existing !== undefined && existing !== roleClass) {
        throw new Error(`${platform} roles normalized to ${key} map to both ${existing} and ${roleClass}`);
      }
      table.set(key, roleClass);
    }
  }
  return table;
}

export const ROLE_TABLES: Readonly<Record<Platform, ReadonlyMap<string, RoleClass>>> = {
  macos: normalizedTable('macos'),
  windows: normalizedTable('windows'),
  linux: normalizedTable('linux'),
};

/** Return the role class for a raw Driver role, or null when excluded. */
// Window-chrome containers, by normalized role. Their actionable descendants
// (Windows' title-bar System menu, Minimize, Maximize, and Close) belong to the
// window manager, not the application, so they are never candidates. Mirrors
// python/native_roles.py WINDOW_CHROME_ROLES.
export const WINDOW_CHROME_ROLES: Readonly<Record<Platform, ReadonlySet<string>>> = {
  macos: new Set(),
  windows: new Set([normalizedRole('TitleBar')]),
  linux: new Set(),
};

export function isWindowChrome(rawRole: unknown, platform: Platform): boolean {
  return typeof rawRole === 'string' && WINDOW_CHROME_ROLES[platform].has(normalizedRole(rawRole));
}

export function roleClass(rawRole: unknown, platform: Platform): RoleClass | null {
  if (typeof rawRole !== 'string' || !rawRole) return null;
  return ROLE_TABLES[platform].get(normalizedRole(rawRole)) ?? null;
}
