// Copies `text` to the clipboard (handled by theme/scripts/copy.ts).
// Styles live in global.css (.copy-btn) so the docs code frames match.
import type { ButtonHTMLAttributes } from 'react';
import { cx } from '../../lib/cx';
import { Icon } from './Icon';

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  text: string;
  label?: string;
  /** `chip`: small mono chip for code windows. `button`: chunky yellow button. */
  variant?: 'chip' | 'button';
};

export function CopyButton({ text, label = 'Copy', variant = 'chip', className, ...rest }: Props) {
  return (
    <button
      type="button"
      className={cx('copy-btn', `copy-btn--${variant}`, className)}
      data-copy={text}
      aria-label={`${label}: ${text}`}
      title="Copy to clipboard"
      {...rest}
    >
      <span className="copy-btn__face">
        <Icon name="copy" size={16} className="copy-btn__icon copy-btn__icon--idle" />
        <Icon name="check" size={16} className="copy-btn__icon copy-btn__icon--done" />
        <span data-copy-label="">{label}</span>
      </span>
    </button>
  );
}
