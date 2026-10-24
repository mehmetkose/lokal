// Docs page shell: green title band, then sidebar | content | "On this page".
// The surrounding Layout (header, <main>, footer, head tags) is the caller's.
import { type ReactNode, useMemo } from 'react';
import { DocsCards } from '../components/docs/DocsCards';
import { DocsHero } from '../components/docs/DocsHero';
import { DocsPager } from '../components/docs/DocsPager';
import { DocsSidebar } from '../components/docs/DocsSidebar';
import { DocsSource } from '../components/docs/DocsSource';
import { DocsToc } from '../components/docs/DocsToc';
import { docSections } from '../data/docs';
import { getDocsNav, getPrevNext } from '../lib/docs';
import { useCurrentDoc, useDocs } from '../lib/useDocs';
import './DocsLayout.css';

type Props = { children: ReactNode };

export function DocsLayout({ children }: Props) {
  const docs = useDocs();
  const { href: current, data, toc: headings } = useCurrentDoc();
  const nav = useMemo(() => getDocsNav(docs), [docs]);
  const { prev, next } = getPrevNext(docs, current);
  const toc = headings.filter((h) => h.depth === 2 || h.depth === 3);
  const isIndex = current === '/docs';

  return (
    <>
      <DocsHero title={data.title} description={data.description} section={docSections[data.section]} />

      <div className="container docs">
        <aside className="docs__sidebar">
          <DocsSidebar nav={nav} current={current} />
        </aside>

        <article className="docs__main">
          {toc.length > 1 && <DocsToc headings={toc} variant="inline" />}
          <div className="prose docs-prose" data-docs-content="">
            {children}
          </div>
          {isIndex && <DocsCards nav={nav} />}
          <DocsSource source={data.source ?? []} />
          <DocsPager prev={prev} next={next} />
        </article>

        <aside className="docs__toc">{toc.length > 1 && <DocsToc headings={toc} variant="side" />}</aside>
      </div>
    </>
  );
}
