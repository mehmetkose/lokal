// One-line install command with a copy button: `$ brew install mehmetkose/tap/lokal`.
import { install } from '../../data/site';
import { cx } from '../../lib/cx';
import { CopyButton } from './CopyButton';
import './InstallSnippet.css';

type Props = {
  command?: string;
  /** `dark` on green/white backgrounds, `light` on orange/yellow ones. */
  variant?: 'dark' | 'light';
  className?: string;
};

export function InstallSnippet({ command = install.brew, variant = 'dark', className }: Props) {
  return (
    <div className={cx('install', `install--${variant}`, className)}>
      <code className="install__cmd">
        <span className="install__prompt" aria-hidden="true">
          $
        </span>
        <span className="install__text">{command}</span>
      </code>
      <CopyButton text={command} variant="button" />
    </div>
  );
}
