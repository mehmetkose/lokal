// lokal logo: CSS padlock + lowercase wordmark, both carrying the template's
// offset dark "shadow" (one drop-shadow on the whole logo).
import type { CSSProperties } from 'react';
import { cx } from '../../lib/cx';
import { LogoMark } from './LogoMark';
import { withBase } from '../../lib/url';
import './Logo.css';

type Props = {
  size?: number;
  /** Render as plain text instead of a link to the home page (Astro's `static`). */
  isStatic?: boolean;
  className?: string;
};

export function Logo({ size = 34, isStatic = false, className }: Props) {
  const Tag = isStatic ? 'span' : 'a';
  return (
    <Tag
      href={isStatic ? undefined : withBase('/')}
      className={cx('logo', className)}
      style={{ '--logo-size': `${size}px` } as CSSProperties}
      aria-label={isStatic ? undefined : 'lokal home'}
    >
      <LogoMark className="logo__mark" />
      <span className="logo__text">lokal</span>
    </Tag>
  );
}
