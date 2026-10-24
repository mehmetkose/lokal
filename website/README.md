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
section: start        # start, guides, reference, concepts, help or project
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

## Social image

`site/public/og.png` (1200×630) and `site/public/apple-touch-icon.png` (180×180) are screenshots of `/brand`. After
changing the logo or tagline, open `/brand` and screenshot `#og-card` and `#touch-icon` at device pixel ratio 1.
