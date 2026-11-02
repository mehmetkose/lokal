# lokal website (Rspress)

The website for [lokal](https://github.com/mehmetkose/lokal): the landing page and the docs, published at
https://mehmetkose.github.io/lokal/. It's an [Rspress](https://rspress.rs) site with a custom theme, prerendered to
static HTML. It is a port of the old Astro site (`lokal-project/web/astro`, outside this repo), with the same design.

## Publishing

`.github/workflows/website.yml` (at the repo root) builds the site on every push to `main` that touches `website/`
and deploys `dist/` to GitHub Pages. Pull requests only build it, which catches dead links. Pages must use
**GitHub Actions** as its source (repo Settings → Pages).

The site is served under a base path, `/lokal/`. The origin and base are set once, in `theme/data/site.ts`; to
move the site to its own domain, set `SITE_ORIGIN` to it and `BASE` to `'/'`.

## Commands

Run these from this folder.

| Command | What it does |
| :-- | :-- |
| `pnpm install` | Install dependencies |
| `pnpm dev` | Dev server at `http://localhost:3000/lokal/` |
| `pnpm build` | Build the site to `./dist/` |
| `pnpm preview` | Build, then serve `./dist/` locally |
| `pnpm typecheck` | Type-check the theme, pages and plugins |

## Layout

```text
site/                          Rspress root: one file per route
├── index.tsx                  Landing page (/)
├── brand.tsx                  Source for public/og.png and apple-touch-icon.png (/brand, noindex)
├── docs.md                    /docs
├── docs/                      Every other docs page in Markdown; the file path is the URL
└── public/                    favicon, social image, robots.txt
theme/
├── index.tsx                  Layout: head tags, header, footer; picks the page shell
├── global.css                 Design tokens, typography, prose and code styles
├── fonts/                     Self-hosted Poppins, Inter and Fira Code (woff2, OFL licenses); fonts.css holds the @font-face rules
├── layouts/DocsLayout.tsx     Docs shell: hero, sidebar, content, "On this page", pager
├── components/
│   ├── layout/                Header, Footer, Logo, LogoMark (the CSS padlock), Seo (head tags)
│   ├── home/ sections/ cards/ Landing page sections
│   ├── docs/                  Docs sidebar, table of contents, pager, …
│   └── ui/                    Button, InstallSnippet, TerminalWindow, CodeBlock, CopyButton, …
├── pages/                     NotFound (404) and Brand
├── data/
│   ├── site.ts                Domain, links, install commands, navigation
│   ├── product.ts             Landing page copy (taken from the lokal README)
│   ├── faqs.ts                Landing page FAQ
│   └── docs.ts                Docs sidebar sections and GitHub links
├── lib/                       Docs ordering (docs.ts), base-path URLs (url.ts), cx, inlineCode, build-time highlighting
└── scripts/                   copy.ts (copy buttons), reveal.ts (scroll reveal)
plugins/
├── rehype-lokal.ts            Code frames + copy buttons, table code, heading anchors, clean links
├── remark-toc-text.ts         Plain-text "On this page" entries
├── lokal-pages.ts             Checks docs frontmatter; writes sitemap.xml
└── drop-default-styles.ts     Removes Rspress's default theme CSS
```

The site origin and base path are set once, in `theme/data/site.ts`. Wrap internal hrefs in components with
`withBase()` from `theme/lib/url.ts`.

## Writing docs

Each page in `site/docs/` (and `site/docs.md`) is Markdown with this frontmatter:

```yaml
---
title: Installing lokal
description: One sentence, used as the subtitle and meta description.
section: start        # start, guides, reference, concepts, compare, help or project
order: 2              # position within the section
sidebarLabel: Install # optional, shorter sidebar label
source: [README.md]   # files in the lokal repo this page is adapted from
draft: true           # optional: leave the page out of the build
---
```

- The sidebar groups pages by `section`, not by folder. The build fails if the frontmatter is invalid.
- Don't add an `# H1`: the title comes from the frontmatter.
- Smart punctuation is on, so put flags and paths in backticks (`--purge`), or `--` becomes a dash.
- Use a `console` code fence for terminal sessions with `$ ` prompts. The copy button copies only the commands.
- Link to other pages by route (`/docs/reference/commands`), not by file name. The build checks links and anchors.

The docs are adapted from the [lokal repo](https://github.com/mehmetkose/lokal) (`README.md` and `docs/`).
When those change, update the matching page here. Each page's `source` lists where it came from, and the
page links back to those files.

## Writing comparison pages

Each comparison of lokal with another tool is one page in `site/docs/compare/` (`localcan.md` is
`/docs/compare/localcan`), with `section: compare`. `site/docs/compare.md` is the overview at `/docs/compare`: it
lists every comparison and the points they all cover. Add a row to its table when you add a page.

Every comparison page has the same parts, in this order:

```markdown
---
title: lokal vs <Tool>
description: One sentence: what both tools do, and how lokal differs.
section: compare
order: 2                       # the overview is 1; new pages go after the existing ones
sidebarLabel: <Tool>
source: [README.md, …]         # the lokal files that lokal's facts on the page come from
---

<Two or three sentences: what both tools do, the main differences, and who should pick which.>

## At a glance
<The comparison table, with the rows below.>

## Free and open source
## Fast
## <One section for each other difference that matters: names, HTTPS, setup, scripting>
## When <Tool> is the better choice
## Moving from <Tool> to lokal
## Sources
```

The "At a glance" table has two columns, lokal and the other tool, and these rows, in this order, so readers can
compare pages: price, source code, platforms, interface, domain names, visible on your network, HTTPS, password,
adding a service, published speed numbers, public URLs, scripting.

Rules for every comparison page:

- **Every fact about the other tool has a source:** its own website, docs, pricing page or changelog. List them
  under Sources with the date you checked them, and check them again when you edit the page.
- **State facts, not verdicts.** Don't call the other tool slow, insecure or expensive. Give its price, what it
  does, and its numbers, and let the reader decide.
- **Speed claims need measurements.** lokal's numbers come from a live test report in `../docs/live-test-reports/`.
  Quote another tool's numbers only if it publishes them, or if you measured both tools on the same machine, and
  say which. With neither, the page says there are no numbers to compare.
- **Say where the other tool is better.** "When <Tool> is the better choice" is required, and it names what lokal
  doesn't do yet.
- **Write the other tool's name the way it does**, and don't use its logo.

## Social image

`site/public/og.png` (1200×630) and `site/public/apple-touch-icon.png` (180×180) are screenshots of `/brand`. After
changing the logo or tagline, open `/brand` and screenshot `#og-card` and `#touch-icon` at device pixel ratio 1.
