// "On this page": a sticky list in the right column on desktop, or a
// collapsible block above the content on smaller screens.
import { useEffect, useRef } from 'react';
import { cx } from '../../lib/cx';
import './DocsToc.css';

type Props = {
  headings: { depth: number; id: string; text: string }[];
  variant: 'side' | 'inline';
};

export function DocsToc({ headings, variant }: Props) {
  const navRef = useRef<HTMLElement>(null);
  const ids = headings.map((h) => h.id).join(' ');

  // Scroll-spy (side list only). The active link is marked through the DOM, so
  // React re-renders never clear it.
  useEffect(() => {
    if (variant !== 'side') return;
    const content = document.querySelector<HTMLElement>('[data-docs-content]');
    const tocLinks = [...(navRef.current?.querySelectorAll<HTMLAnchorElement>('[data-toc-link]') ?? [])];
    if (!content || !tocLinks.length || !('IntersectionObserver' in window)) return;

    const byId = new Map(tocLinks.map((a) => [a.dataset.tocLink!, a]));
    const targets = [...content.querySelectorAll<HTMLElement>('h2[id], h3[id]')].filter((h) => byId.has(h.id));
    const visible = new Set<string>();

    const setActive = () => {
      // The first visible heading wins; otherwise keep the last one scrolled past.
      const first = targets.find((h) => visible.has(h.id));
      let id = first?.id;
      if (!id) {
        const above = targets.filter((h) => h.getBoundingClientRect().top < window.innerHeight * 0.3);
        id = above.at(-1)?.id;
      }
      tocLinks.forEach((a) => a.classList.toggle('is-active', a.dataset.tocLink === id));

      // On long pages the list scrolls on its own: keep the active link in view.
      const link = id ? byId.get(id) : undefined;
      const box = link?.closest<HTMLElement>('.docs__toc');
      if (link && box && box.scrollHeight > box.clientHeight) {
        const b = box.getBoundingClientRect();
        const r = link.getBoundingClientRect();
        if (r.top < b.top + 40 || r.bottom > b.bottom - 40) {
          box.scrollTo({ top: box.scrollTop + r.top - b.top - b.height / 3 });
        }
      }
    };

    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((e) => (e.isIntersecting ? visible.add(e.target.id) : visible.delete(e.target.id)));
        setActive();
      },
      { rootMargin: '-120px 0px -55% 0px' },
    );
    targets.forEach((h) => io.observe(h));
    return () => {
      io.disconnect();
      tocLinks.forEach((a) => a.classList.remove('is-active'));
    };
  }, [variant, ids]);

  return variant === 'side' ? (
    <nav className="toc toc--side" aria-label="On this page" data-docs-toc="" ref={navRef}>
      <p className="toc__heading">On this page</p>
      <ul className="toc__list">
        {headings.map((h) => (
          <li className={cx('toc__item', `toc__item--${h.depth}`)} key={h.id}>
            <a href={`#${h.id}`} className="toc__link" data-toc-link={h.id}>
              {h.text}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  ) : (
    <details className="toc toc--inline">
      <summary className="toc__summary">On this page</summary>
      <ul className="toc__list">
        {headings.map((h) => (
          <li className={cx('toc__item', `toc__item--${h.depth}`)} key={h.id}>
            <a href={`#${h.id}`} className="toc__link">
              {h.text}
            </a>
          </li>
        ))}
      </ul>
    </details>
  );
}
