// Checks every docs page's frontmatter (the Astro site validated it with zod)
// and writes sitemap.xml: the home page plus the docs, in sidebar order.
// /brand (noindex) and the 404 page stay out, as before. Also removes the
// search index Rspress 2.0 always writes: the site has no search.
import { copyFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { RspressPlugin } from '@rspress/core';
import { absoluteUrl } from '../theme/lib/url';
import { docFrontmatterErrors, getDocs, isDocRoute } from '../theme/lib/docs';

export function lokalPages(): RspressPlugin {
  const pages = new Map<string, { routePath: string; frontmatter: Record<string, unknown> }>();

  return {
    name: 'lokal-pages',

    extendPageData(pageData) {
      if (!isDocRoute(pageData.routePath)) return;
      const errors = docFrontmatterErrors(pageData.frontmatter);
      if (errors.length) {
        throw new Error(`Invalid frontmatter in ${pageData._relativePath}:\n  ${errors.join('\n  ')}`);
      }
      pages.set(pageData.routePath, { routePath: pageData.routePath, frontmatter: pageData.frontmatter });
    },

    async afterBuild(config) {
      const paths = ['/', ...getDocs([...pages.values()]).map((doc) => doc.href)];
      const urls = paths.map((p) => `  <url><loc>${absoluteUrl(p)}</loc></url>`).join('\n');
      const body = `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${urls}
</urlset>
`;
      const outDir = path.resolve(config.outDir ?? 'doc_build');
      await writeFile(path.join(outDir, 'sitemap.xml'), body);

      // A page with a folder of the same name (docs.html and docs/, docs/compare.html
      // and docs/compare/): GitHub Pages may redirect /lokal/docs to /lokal/docs/, so
      // the folder needs an index.html copy of the page too.
      for (const p of paths.filter((p) => p !== '/')) {
        const dir = path.join(outDir, p);
        const isDir = await stat(dir).then((s) => s.isDirectory(), () => false);
        if (isDir) await copyFile(`${dir}.html`, path.join(dir, 'index.html'));
      }

      const staticDir = path.join(outDir, 'static');
      for (const file of await readdir(staticDir)) {
        if (/^search_index\b.*\.json$/.test(file)) await rm(path.join(staticDir, file));
      }
    },
  };
}
