// Docs navigation, grouped by section. On phones it folds behind a toggle.
import { useState } from 'react';
import { githubDocs } from '../../data/docs';
import { cx } from '../../lib/cx';
import type { DocsNavGroup } from '../../lib/docs';
import { Icon } from '../ui/Icon';
import { withBase } from '../../lib/url';
import './DocsSidebar.css';

type Props = {
  nav: DocsNavGroup[];
  current: string;
};

export function DocsSidebar({ nav, current }: Props) {
  const currentLabel = nav.flatMap((g) => g.items).find((i) => i.href === current)?.label ?? 'Docs';
  // Phone nav toggle. Starts closed, as in the server markup.
  const [open, setOpen] = useState(false);

  return (
    <div className={cx('sidebar', open && 'is-open')} data-docs-sidebar="">
      <button
        className="sidebar__toggle"
        aria-expanded={open}
        aria-controls="docs-nav"
        data-docs-nav-toggle=""
        onClick={() => setOpen((o) => !o)}
      >
        <span className="sidebar__toggle-label">
          <Icon name="book" size={18} />
          <span>Docs menu</span>
          <span className="sidebar__toggle-current">{`· ${currentLabel}`}</span>
        </span>
        <Icon name="chevron-down" size={18} className="sidebar__chevron" />
      </button>

      <nav className="sidebar__nav" id="docs-nav" aria-label="Docs">
        {nav.map((group) => (
          <div className="sidebar__group" key={group.id}>
            <p className="sidebar__heading">{group.label}</p>
            <ul className="sidebar__list">
              {group.items.map((item) => (
                <li key={item.href}>
                  <a
                    href={withBase(item.href)}
                    className="sidebar__link"
                    aria-current={item.href === current ? 'page' : undefined}
                  >
                    {item.label}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        ))}
        <div className="sidebar__group">
          <p className="sidebar__heading">On GitHub</p>
          <ul className="sidebar__list">
            {githubDocs.map((item) => (
              <li key={item.href}>
                <a href={item.href} className="sidebar__link sidebar__link--external" target="_blank" rel="noopener">
                  {item.label}
                  <Icon name="arrow-up-right" size={14} />
                </a>
              </li>
            ))}
          </ul>
        </div>
      </nav>
    </div>
  );
}
