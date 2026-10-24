// The whole page shell. Rspress's default theme UI isn't used: this Layout
// renders the same markup as web/astro/src/layouts/Layout.astro, and docs
// pages get the docs shell (layouts/DocsLayout.tsx).
import { Content, usePage } from '@rspress/core/runtime';
import { type ReactNode, useEffect } from 'react';
import { Footer } from './components/layout/Footer';
import { Header } from './components/layout/Header';
import { Seo } from './components/layout/Seo';
import { DocsLayout } from './layouts/DocsLayout';
import type { DocData } from './lib/docs';
import { NotFound } from './pages/NotFound';
import { installCopy } from './scripts/copy';
import { useReveal } from './scripts/reveal';

function Shell({ children }: { children: ReactNode }) {
  return (
    <>
      <a href="#main" className="skip-link">
        Skip to content
      </a>
      <Header />
      <main id="main">{children}</main>
      <Footer />
    </>
  );
}

function DocsPage() {
  const { page } = usePage();
  const data = page.frontmatter as unknown as DocData;
  return (
    <>
      <Seo title={page.routePath === '/docs' ? 'Docs' : data.title} description={data.description} ogType="article" />
      <Shell>
        <DocsLayout>
          <Content />
        </DocsLayout>
      </Shell>
    </>
  );
}

function Layout() {
  const { page } = usePage();

  useEffect(installCopy, []);
  useReveal(page.routePath);

  switch (page.pageType) {
    // Standalone pages (site/brand.tsx) render everything themselves.
    case 'blank':
      return <Content />;
    case '404':
      return (
        <>
          <Seo title="Page not found" description="This page doesn’t exist." noindex />
          <Shell>
            <NotFound />
          </Shell>
        </>
      );
    // The landing page (site/index.tsx).
    case 'home':
    case 'custom':
      return (
        <>
          <Seo />
          <Shell>
            <Content />
          </Shell>
        </>
      );
    default:
      return <DocsPage />;
  }
}

export { Layout };
// The Rspress runtime also imports Root and useThemeState from the theme.
export * from '@rspress/core/theme-original';
