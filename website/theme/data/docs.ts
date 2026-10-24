// Docs sidebar sections, in display order. A page's frontmatter `section`
// must be one of these ids.
export const sectionIds = ['start', 'guides', 'reference', 'concepts', 'help', 'project'] as const;
export type SectionId = (typeof sectionIds)[number];

export const docSections: Record<SectionId, string> = {
  start: 'Getting started',
  guides: 'Guides',
  reference: 'Reference',
  concepts: 'How it works',
  help: 'Help',
  project: 'Project',
};

const blob = 'https://github.com/mehmetkose/lokal/blob/main';

// Contributor docs stay in the repo; the sidebar links to them on GitHub.
export const githubDocs = [
  { label: 'Specification', href: `${blob}/docs/SPEC.md` },
  { label: 'Architecture', href: `${blob}/docs/ARCHITECTURE.md` },
  { label: 'Testing', href: `${blob}/docs/TESTING.md` },
  { label: 'Porting', href: `${blob}/docs/PORTING.md` },
  { label: 'Packaging', href: `${blob}/docs/PACKAGING.md` },
  { label: 'Releasing', href: `${blob}/docs/RELEASING.md` },
  { label: 'Contributing', href: `${blob}/CONTRIBUTING.md` },
  { label: 'How v0.1 was built', href: `${blob}/docs/history/README.md` },
];

export const sourceUrl = (path: string) => `${blob}/${path}`;
