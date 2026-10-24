// Section cards on the docs index page.
import type { DocsNavGroup } from '../../lib/docs';
import { withBase } from '../../lib/url';
import './DocsCards.css';

type Props = {
  nav: DocsNavGroup[];
};

export function DocsCards({ nav }: Props) {
  // The index itself is the first "Getting started" page; leave it out.
  const groups = nav
    .map((g) => ({ ...g, items: g.items.filter((i) => i.href !== '/docs') }))
    .filter((g) => g.items.length > 0);

  return (
    <section className="cards" aria-label="All docs">
      {groups.map((group) => (
        <div className="cards__group" key={group.id}>
          <h2 className="cards__heading">{group.label}</h2>
          <ul className="cards__grid">
            {group.items.map((item) => (
              <li key={item.href}>
                <a href={withBase(item.href)} className="doc-card">
                  <span className="doc-card__title">{item.title}</span>
                  <span className="doc-card__desc">{item.description}</span>
                </a>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
