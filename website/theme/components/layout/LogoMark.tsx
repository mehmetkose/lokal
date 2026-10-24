// The lokal padlock, drawn in CSS only: the element is the shackle, ::after is
// the body with a keyhole cut from two gradients. Everything is sized in em,
// so `size` (px) or the surrounding font-size scales it.
import type { HTMLAttributes } from 'react';
import { cx } from '../../lib/cx';
import './LogoMark.css';

type Props = Omit<HTMLAttributes<HTMLSpanElement>, 'color'> & {
  size?: number;
  /** Padlock colour. */
  color?: 'yellow' | 'white' | 'green';
  className?: string;
};

// Extra props are forwarded so a parent's styles can target the mark via `className`.
export function LogoMark({ size, color = 'yellow', className, ...rest }: Props) {
  return (
    <span
      className={cx('mark', `mark--${color}`, className)}
      style={size ? { fontSize: `${size}px` } : undefined}
      aria-hidden="true"
      {...rest}
    ></span>
  );
}
