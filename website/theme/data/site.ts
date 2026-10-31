// Site-wide settings: domain, project links, install commands and navigation.
// The site's address lives here only; rspress.config.ts and theme/lib/url.ts import it.
// GitHub Pages serves it under a subpath: https://mehmetkose.github.io/lokal/.

import type { IconName } from '../components/ui/icons';

export const SITE_ORIGIN = 'https://mehmetkose.github.io';
export const BASE = '/lokal/';
export const SITE_URL = `${SITE_ORIGIN}/lokal`;

const repo = 'https://github.com/mehmetkose/lokal';

export const site = {
  name: 'lokal',
  title: 'lokal — real HTTPS domains for your local dev servers',
  tagline: 'Real HTTPS domains for your local dev servers, on macOS.',
  description:
    'lokal gives every service you run in development its own https://<name>.lokal URL with a certificate your browser, curl, Node and Python trust. Set it up once; adding a service takes milliseconds and never asks for a password.',
  version: '0.5.1',
  author: 'Mehmet Kose',
  license: 'Apache-2.0',
  requires: 'macOS 14 Sonoma or later',
};

export const links = {
  repo,
  releases: `${repo}/releases`,
  latestRelease: `${repo}/releases/latest`,
  issues: `${repo}/issues/new/choose`,
  security: `${repo}/security/advisories/new`,
  contributing: `${repo}/blob/main/CONTRIBUTING.md`,
  license: `${repo}#license`,
  tap: 'https://github.com/mehmetkose/homebrew-tap',
};

export const install = {
  brew: 'brew install mehmetkose/tap/lokal',
  setup: 'lokal setup',
};

type NavLink = { label: string; href: string };
type MenuLink = NavLink & { icon: IconName; desc?: string };

// Header: the "Docs" dropdown, laid out in three columns. The featured column
// shows each link with a one-line description.
export const docsMenu: { label: string; featured?: boolean; items: MenuLink[] }[] = [
  {
    label: 'Get started',
    featured: true,
    items: [
      { label: 'Introduction', href: '/docs', icon: 'book', desc: 'HTTPS names for your dev servers' },
      { label: 'Install', href: '/docs/installation', icon: 'download', desc: 'Homebrew, a binary or source' },
      { label: 'Setup', href: '/docs/setup', icon: 'lock', desc: 'Once per Mac, with your password' },
      { label: 'Quick start', href: '/docs/quick-start', icon: 'zap', desc: 'Your first URL in five minutes' },
    ],
  },
  {
    label: 'Guides',
    items: [
      { label: 'Projects & services', href: '/docs/guides/projects-and-services', icon: 'layers' },
      { label: 'Node, Python & curl', href: '/docs/guides/lokal-env', icon: 'braces' },
      { label: 'Frameworks', href: '/docs/guides/frameworks', icon: 'blocks' },
      { label: 'Coding agents', href: '/docs/guides/coding-agents', icon: 'bot' },
      { label: '.lokal or .local', href: '/docs/guides/lokal-vs-local', icon: 'globe' },
    ],
  },
  {
    label: 'Reference & help',
    items: [
      { label: 'Commands', href: '/docs/reference/commands', icon: 'terminal' },
      { label: 'How it works', href: '/docs/concepts/how-it-works', icon: 'network' },
      { label: 'Security model', href: '/docs/concepts/security-model', icon: 'shield' },
      { label: 'Troubleshooting', href: '/docs/troubleshooting', icon: 'wrench' },
      { label: 'FAQ', href: '/docs/faq', icon: 'help' },
    ],
  },
];

// The strip under the dropdown's link columns.
export const docsMenuTip = {
  cmd: 'lokal doctor',
  text: 'checks every layer and prints a fix',
  href: '/docs/troubleshooting',
};

// Header links shown next to the Docs dropdown.
export const mainNav: NavLink[] = [
  { label: 'Quick start', href: '/docs/quick-start' },
  { label: 'Commands', href: '/docs/reference/commands' },
  { label: 'Changelog', href: '/docs/changelog' },
];

// Burger menu on tablet/phone.
export const mobileNav: NavLink[] = [
  { label: 'Home', href: '/' },
  { label: 'Docs', href: '/docs' },
  { label: 'Install', href: '/docs/installation' },
  { label: 'Quick start', href: '/docs/quick-start' },
  { label: 'Commands', href: '/docs/reference/commands' },
  { label: 'Coding agents', href: '/docs/guides/coding-agents' },
  { label: 'Troubleshooting', href: '/docs/troubleshooting' },
  { label: 'FAQ', href: '/docs/faq' },
  { label: 'Changelog', href: '/docs/changelog' },
];

export const footerNav: { label: string; items: (NavLink & { external?: boolean })[] }[] = [
  {
    label: 'Product',
    items: [
      { label: 'Install', href: '/docs/installation' },
      { label: 'Quick start', href: '/docs/quick-start' },
      { label: 'How it works', href: '/docs/concepts/how-it-works' },
      { label: 'Security model', href: '/docs/concepts/security-model' },
      { label: 'Performance', href: '/docs/concepts/performance' },
      { label: 'Roadmap', href: '/docs/roadmap' },
    ],
  },
  {
    label: 'Docs',
    items: [
      { label: 'Commands', href: '/docs/reference/commands' },
      { label: 'Projects & services', href: '/docs/guides/projects-and-services' },
      { label: 'Node, Python & curl', href: '/docs/guides/lokal-env' },
      { label: 'Frameworks', href: '/docs/guides/frameworks' },
      { label: 'Coding agents', href: '/docs/guides/coding-agents' },
      { label: 'Troubleshooting', href: '/docs/troubleshooting' },
    ],
  },
  {
    label: 'Project',
    items: [
      { label: 'GitHub', href: links.repo, external: true },
      { label: 'Releases', href: links.releases, external: true },
      { label: 'Homebrew tap', href: links.tap, external: true },
      { label: 'Changelog', href: '/docs/changelog' },
      { label: 'Contributing', href: links.contributing, external: true },
      { label: 'Report a vulnerability', href: links.security, external: true },
    ],
  },
];
