// Compact green title band at the top of every docs page.
import { TornEdge } from '../ui/TornEdge';
import { withBase } from '../../lib/url';
import './DocsHero.css';

type Props = {
  title: string;
  description: string;
  section: string;
};

// Keep TLDs such as `.lokal` lowercase inside the uppercase heading.
const escape = (t: string) => t.replace(/&/g, '&amp;').replace(/</g, '&lt;');

export function DocsHero({ title, description, section }: Props) {
  const titleHtml = escape(title).replace(/(^|\s)(\.[a-z][\w-]*)/g, '$1<span class="nocase">$2</span>');

  return (
    <section className="docs-hero bg-green">
      <div className="container docs-hero__inner">
        <p className="subtitle c-yellow">
          <span aria-hidden="true">{'// '}</span>
          <a href={withBase('/docs')}>docs</a>
          {` / ${section.toLowerCase()}`}
        </p>
        <h1 className="h2 c-white docs-hero__title" dangerouslySetInnerHTML={{ __html: titleHtml }} />
        <p className="docs-hero__desc">{description}</p>
      </div>
      <TornEdge position="bottom" color="green" />
    </section>
  );
}
