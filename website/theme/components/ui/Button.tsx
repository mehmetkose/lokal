// Chunky "3D" button: a colored face sitting on a darker base, with the
// label rolling up on hover (two stacked copies of the text).
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import { withBase } from '../../lib/url';
import './Button.css';

type Props = Omit<AnchorHTMLAttributes<HTMLElement>, 'type'> & {
  href?: string;
  type?: 'button' | 'submit';
  variant?: 'yellow' | 'orange' | 'green';
  /** Color of the base layer underneath the face. */
  base?: 'dark' | 'green';
  /** Label color override. */
  textColor?: 'dark' | 'green' | 'white';
  full?: boolean;
  children: ReactNode;
};

const defaultText = { yellow: 'dark', orange: 'white', green: 'white' } as const;

export function Button({
  href,
  type = 'button',
  variant = 'yellow',
  base = 'dark',
  textColor,
  full = false,
  className,
  children,
  ...rest
}: Props) {
  const color = textColor ?? defaultText[variant];
  const classes = ['btn', `btn--${variant}`, `btn--base-${base}`, `btn--text-${color}`, full && 'btn--full', className]
    .filter(Boolean)
    .join(' ');
  const face = (
    <span className="btn__face">
      <span className="btn__label">
        <span className="btn__roll">
          <span>{children}</span>
          <span aria-hidden="true">{children}</span>
        </span>
      </span>
    </span>
  );
  return href ? (
    <a className={classes} href={withBase(href)} {...rest}>
      {face}
    </a>
  ) : (
    <button className={classes} type={type} {...(rest as object)}>
      {face}
    </button>
  );
}
