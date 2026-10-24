// A browser address bar showing a padlock and an https URL.
import type { HTMLAttributes } from 'react';
import { cx } from '../../lib/cx';
import { Icon } from './Icon';
import './BrowserBar.css';

type Props = HTMLAttributes<HTMLDivElement> & { url: string };

export function BrowserBar({ url, className, ...rest }: Props) {
  const { protocol, host, pathname } = new URL(url);
  return (
    <div className={cx('browser', className)} {...rest}>
      <span className="browser__dots" aria-hidden="true">
        <span></span>
        <span></span>
        <span></span>
      </span>
      <span className="browser__address">
        <Icon name="lock" size={16} className="browser__lock" />
        <span className="browser__url">
          <span className="browser__scheme">{`${protocol}//`}</span>
          <strong>{host}</strong>
          {pathname !== '/' && pathname}
        </span>
      </span>
    </div>
  );
}
