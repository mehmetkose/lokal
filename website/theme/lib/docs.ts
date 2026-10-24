// Docs pages in sidebar order, built from each page's frontmatter (not from
// folders). Pure functions: used by the theme at runtime (see useDocs.ts) and
// by plugins/lokal-pages.ts at build time.
import { docSections, sectionIds, type SectionId } from '../data/docs';

export type DocData = {
  title: string;
  description: string;
  section: SectionId;
  /** Position within its section (lower comes first). */
  order: number;
  /** Shorter label for the sidebar, when the title is long. */
  sidebarLabel?: string;
  /** Files in the lokal repo this page is adapted from, e.g. "README.md". */
  source?: string[];
  draft?: boolean;
};

export type Doc = { href: string; data: DocData };

export type DocsNavGroup = {
  id: SectionId;
  label: string;
  items: { label: string; title: string; description: string; href: string }[];
};

type PageLike = { routePath: string; frontmatter?: Record<string, unknown> };

/** Docs routes: site/docs.md is /docs, site/docs/guides/x.md is /docs/guides/x. */
export const isDocRoute = (routePath: string) => routePath === '/docs' || routePath.startsWith('/docs/');

/** Problems with a docs page's frontmatter (the Astro version validated it with zod). */
export function docFrontmatterErrors(frontmatter: Record<string, unknown> = {}): string[] {
  const errors: string[] = [];
  const { title, description, section, order, sidebarLabel, source, draft } = frontmatter;
  if (typeof title !== 'string') errors.push('`title` must be a string');
  if (typeof description !== 'string') errors.push('`description` must be a string');
  if (!sectionIds.includes(section as SectionId)) errors.push(`\`section\` must be one of ${sectionIds.join(', ')}`);
  if (typeof order !== 'number') errors.push('`order` must be a number');
  if (sidebarLabel !== undefined && typeof sidebarLabel !== 'string') errors.push('`sidebarLabel` must be a string');
  if (source !== undefined && !(Array.isArray(source) && source.every((s) => typeof s === 'string')))
    errors.push('`source` must be a list of strings');
  if (draft !== undefined && typeof draft !== 'boolean') errors.push('`draft` must be true or false');
  return errors;
}

/** Published docs in sidebar order: by section, then by `order`. */
export function getDocs(pages: PageLike[]): Doc[] {
  return pages
    .filter((p) => isDocRoute(p.routePath) && p.frontmatter && !p.frontmatter.draft)
    .map((p) => ({ href: p.routePath, data: p.frontmatter as unknown as DocData }))
    .sort(
      (a, b) =>
        sectionIds.indexOf(a.data.section) - sectionIds.indexOf(b.data.section) || a.data.order - b.data.order,
    );
}

export function getDocsNav(docs: Doc[]): DocsNavGroup[] {
  return sectionIds
    .map((id) => ({
      id,
      label: docSections[id],
      items: docs
        .filter((d) => d.data.section === id)
        .map((d) => ({
          label: d.data.sidebarLabel ?? d.data.title,
          title: d.data.title,
          description: d.data.description,
          href: d.href,
        })),
    }))
    .filter((group) => group.items.length > 0);
}

/** The pages before and after `href` in sidebar order. */
export function getPrevNext(docs: Doc[], href: string) {
  const i = docs.findIndex((d) => d.href === href);
  const link = (d?: Doc) => d && { title: d.data.sidebarLabel ?? d.data.title, href: d.href };
  return { prev: link(docs[i - 1]), next: link(docs[i + 1]) };
}
