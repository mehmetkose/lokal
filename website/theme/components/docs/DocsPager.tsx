// Previous / next page cards at the bottom of a docs page.
import { Icon } from '../ui/Icon';
import { withBase } from '../../lib/url';
import './DocsPager.css';

type Link = { title: string; href: string } | undefined;

type Props = {
  prev: Link;
  next: Link;
};

export function DocsPager({ prev, next }: Props) {
  return (
    <nav className="pager" aria-label="Previous and next page">
      {prev ? (
        <a href={withBase(prev.href)} className="pager__card pager__card--prev" rel="prev">
          <span className="pager__label">
            <Icon name="arrow-left" size={14} /> previous
          </span>
          <span className="pager__title">{prev.title}</span>
        </a>
      ) : (
        <span />
      )}
      {next && (
        <a href={withBase(next.href)} className="pager__card pager__card--next" rel="next">
          <span className="pager__label">
            next <Icon name="arrow-right" size={14} />
          </span>
          <span className="pager__title">{next.title}</span>
        </a>
      )}
    </nav>
  );
}
