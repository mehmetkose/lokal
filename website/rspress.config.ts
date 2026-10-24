import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from '@rspress/core';
import remarkSmartypants from 'remark-smartypants';
import { codeToHtml, createCssVariablesTheme } from 'shiki';
import { dropDefaultStyles } from './plugins/drop-default-styles';
import { lokalPages } from './plugins/lokal-pages';
import { rehypeLokal } from './plugins/rehype-lokal';
import { remarkTocText } from './plugins/remark-toc-text';
import { agentsSnippet } from './theme/data/product';
import { BASE, SITE_ORIGIN, site } from './theme/data/site';

// Code colours come from the --astro-code-* tokens in theme/global.css, the
// same Shiki theme the Astro site used.
const codeTheme = createCssVariablesTheme({
  name: 'css-variables',
  variablePrefix: '--astro-code-',
  variableDefaults: {},
  fontStyle: true,
});

// Code blocks rendered by components (CodeBlock), highlighted here at build time.
const highlight = [{ code: agentsSnippet, lang: 'markdown' }];

async function highlightAll() {
  const entries = await Promise.all(
    highlight.map(async ({ code, lang }) => {
      const html = await codeToHtml(code.trim(), { lang, theme: codeTheme });
      const inner = html.match(/<code>([\s\S]*)<\/code><\/pre>$/);
      if (!inner) throw new Error(`Unexpected Shiki output for:\n${code}`);
      return [code.trim(), inner[1]] as const;
    }),
  );
  return Object.fromEntries(entries);
}

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, 'site');

// Pages with `draft: true` in their frontmatter aren't built at all, as on the
// Astro site. Returns their paths relative to `root`, for `route.exclude`.
async function draftPages() {
  const files = (await readdir(root, { recursive: true })).filter((file) => file.endsWith('.md'));
  const drafts = await Promise.all(
    files.map(async (file) => {
      const frontmatter = (await readFile(path.join(root, file), 'utf8')).match(/^---\r?\n([\s\S]*?)\r?\n---/);
      return frontmatter && /^draft:\s*true\s*$/m.test(frontmatter[1]) ? file.split(path.sep).join('/') : [];
    }),
  );
  return drafts.flat();
}

export default defineConfig(async () => ({
  root,
  outDir: 'dist',
  title: site.name,
  description: site.description,
  // GitHub Pages serves the site at https://mehmetkose.github.io/lokal/.
  base: BASE,
  siteOrigin: SITE_ORIGIN,
  // Emit /docs/quick-start.html and link to /docs/quick-start: GitHub Pages serves it without a redirect.
  route: { cleanUrls: true, exclude: await draftPages() },
  globalStyles: path.join(here, 'theme/global.css'),
  // Light only, no search, no default theme UI: theme/index.tsx renders the whole page.
  themeConfig: { darkMode: false },
  search: false,
  llms: false,
  markdown: {
    // Smart punctuation, as in Astro's Markdown pipeline. Put flags in backticks.
    remarkPlugins: [remarkSmartypants, remarkTocText],
    rehypePlugins: [rehypeLokal],
    shiki: { theme: codeTheme },
    link: { checkDeadLinks: true, checkAnchors: true },
  },
  plugins: [lokalPages()],
  builderConfig: {
    plugins: [dropDefaultStyles()],
    source: {
      define: { __LOKAL_HIGHLIGHTED__: JSON.stringify(await highlightAll()) },
    },
    // Fonts are self-hosted (theme/fonts/). Preload the two every page shows above the fold.
    performance: {
      preload: { type: 'all-assets', include: [/\/(?:inter|poppins-800)-latin\.[^./]+\.woff2$/] },
    },
    html: {
      // The per-page <title> comes from theme/index.tsx.
      title: '',
      // Rsbuild adds the base to root paths here: '/favicon.svg' becomes '/lokal/favicon.svg'.
      tags: [
        { tag: 'link', attrs: { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' } },
        { tag: 'link', attrs: { rel: 'apple-touch-icon', href: '/apple-touch-icon.png' } },
        { tag: 'link', attrs: { rel: 'sitemap', href: '/sitemap.xml' } },
        { tag: 'meta', attrs: { name: 'theme-color', content: '#18623b' } },
        { tag: 'meta', attrs: { name: 'color-scheme', content: 'light' } },
        // Hides [data-reveal] content until it scrolls into view (see useReveal).
        { tag: 'script', children: "document.documentElement.classList.add('js');" },
      ],
    },
  },
}));
