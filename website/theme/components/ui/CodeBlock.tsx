// A highlighted code block in a window frame, with a filename and copy button.
// The code is highlighted at build time (lib/highlighted.ts); the <pre> matches
// what Astro's <Code theme="css-variables"> rendered.
import type { HTMLAttributes } from 'react';
import { cx } from '../../lib/cx';
import { highlighted } from '../../lib/highlighted';
import { CopyButton } from './CopyButton';

type Props = HTMLAttributes<HTMLDivElement> & {
  code: string;
  lang?: string;
  filename?: string;
  copy?: boolean;
};

export function CodeBlock({ code, lang = 'text', filename, copy = true, className, ...rest }: Props) {
  return (
    <div className={cx('code-window', className)} {...rest}>
      <div className="code-window__bar">
        <span className="code-window__dots" aria-hidden="true">
          <span></span>
          <span></span>
          <span></span>
        </span>
        <span className="code-window__title">{filename ?? lang}</span>
        {copy && <CopyButton text={code.trim()} />}
      </div>
      <pre
        className="astro-code css-variables"
        style={{
          backgroundColor: 'var(--astro-code-background)',
          color: 'var(--astro-code-foreground)',
          overflowX: 'auto',
        }}
        tabIndex={0}
        data-language={lang}
        dangerouslySetInnerHTML={{ __html: `<code>${highlighted(code.trim())}</code>` }}
      />
    </div>
  );
}
