import { BASE, SITE_ORIGIN } from '../data/site';

// The site is served under BASE (/lokal/), but pages and nav data use root paths
// like '/docs'. withBase('/docs') → '/lokal/docs'. External URLs, '#…' and relative
// hrefs pass through unchanged (Rspress's own withBase would prefix https:// too).
// Markdown links don't need this: Rspress adds the base to them.
export const withBase = (href: string) => (href.startsWith('/') && !href.startsWith('//') ? BASE + href.slice(1) : href);

/** A root path as a full URL, for canonical links, social cards and the sitemap. */
export const absoluteUrl = (path: string) => SITE_ORIGIN + withBase(path);
