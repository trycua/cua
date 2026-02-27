export interface DocsSiteConfig {
  name: string;
  slug: string;
  label: string;
  href: string;
  prefix: string;
  isDefault: boolean;
  description: string;
  iconWidth: number;
  iconHeight: number;
  dropdownIconWidth: number;
  dropdownIconHeight: number;
  navTabs: { name: string; href: string; prefix: string }[];
}

export const docsSites: DocsSiteConfig[] = [
  {
    name: 'Cua',
    slug: 'cua',
    label: 'Docs',
    href: '/cua/guide/get-started/what-is-cua',
    prefix: '/cua',
    isDefault: true,
    description: 'Computer Use Agent SDK',
    iconWidth: 24,
    iconHeight: 24,
    dropdownIconWidth: 28,
    dropdownIconHeight: 28,
    navTabs: [
      { name: 'Guide', href: '/cua/guide/get-started/what-is-cua', prefix: '/cua/guide' },
      { name: 'Examples', href: '/cua/examples/automation/form-filling', prefix: '/cua/examples' },
      { name: 'Reference', href: '/cua/reference/computer-sdk', prefix: '/cua/reference' },
    ],
  },
  {
    name: 'Cua Bench',
    slug: 'cuabench',
    label: 'Docs',
    href: '/cuabench/guide/getting-started/introduction',
    prefix: '/cuabench',
    isDefault: false,
    description: 'Benchmarking toolkit',
    iconWidth: 36,
    iconHeight: 22,
    dropdownIconWidth: 42,
    dropdownIconHeight: 25,
    navTabs: [
      {
        name: 'Guide',
        href: '/cuabench/guide/getting-started/introduction',
        prefix: '/cuabench/guide',
      },
      {
        name: 'Examples',
        href: '/cuabench/examples/custom-agent',
        prefix: '/cuabench/examples',
      },
      {
        name: 'Reference',
        href: '/cuabench/reference/cli-reference',
        prefix: '/cuabench/reference',
      },
    ],
  },
  {
    name: 'Cua-Bot',
    slug: 'cuabot',
    label: 'Docs',
    href: '/cuabot/guide/getting-started/introduction',
    prefix: '/cuabot',
    isDefault: false,
    description: 'Co-op computer-use for any agent',
    iconWidth: 24,
    iconHeight: 24,
    dropdownIconWidth: 28,
    dropdownIconHeight: 28,
    navTabs: [
      {
        name: 'Guide',
        href: '/cuabot/guide/getting-started/introduction',
        prefix: '/cuabot/guide',
      },
      {
        name: 'Reference',
        href: '/cuabot/reference',
        prefix: '/cuabot/reference',
      },
    ],
  },
  {
    name: 'Lume',
    slug: 'lume',
    label: 'Docs',
    href: '/lume/guide/getting-started/introduction',
    prefix: '/lume',
    isDefault: false,
    description: 'macOS VM CLI and Framework',
    iconWidth: 24,
    iconHeight: 24,
    dropdownIconWidth: 28,
    dropdownIconHeight: 28,
    navTabs: [
      {
        name: 'Guide',
        href: '/lume/guide/getting-started/introduction',
        prefix: '/lume/guide',
      },
      {
        name: 'Examples',
        href: '/lume/examples',
        prefix: '/lume/examples',
      },
      {
        name: 'Reference',
        href: '/lume/reference/cli-reference',
        prefix: '/lume/reference',
      },
    ],
  },
];

export const sidebarPages = docsSites.map((site) => site.slug);
