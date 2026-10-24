// A terminal window with a typed session. Only show output the CLI really
// prints (README.md or crates/lokal/src/cli). The copy button copies the
// commands only, one per line.
import type { CSSProperties, HTMLAttributes } from 'react';
import { cx } from '../../lib/cx';
import { CopyButton } from './CopyButton';
import type { TerminalLine } from './terminal';
import './TerminalWindow.css';

export type { TerminalLine };

type Props = HTMLAttributes<HTMLDivElement> & {
  lines: TerminalLine[];
  title?: string;
  copy?: boolean;
  /** Rotation in degrees, for the playful tilted look on the home page. */
  tilt?: number;
};

// Split a leading ✓ / ✗ / ! so it can be coloured on its own.
const mark = (text: string) => {
  const m = text.match(/^(\s*)([✓✗!])(\s.*)$/);
  return m ? { lead: m[1], sym: m[2], rest: m[3] } : null;
};

export function TerminalWindow({ lines, title = 'zsh', copy = true, tilt, className, ...rest }: Props) {
  // Line up trailing comments like the README does.
  const commented = lines.filter((l) => l.cmd !== undefined && l.comment);
  const pad = commented.length ? Math.max(...commented.map((l) => l.cmd!.length)) + 2 : 0;
  const commands = lines
    .filter((l) => l.cmd !== undefined)
    .map((l) => l.cmd)
    .join('\n');

  return (
    <div
      className={cx('code-window', 'term', className)}
      style={tilt ? ({ '--tilt': `${tilt}deg` } as CSSProperties) : undefined}
      {...rest}
    >
      <div className="code-window__bar">
        <span className="code-window__dots" aria-hidden="true">
          <span></span>
          <span></span>
          <span></span>
        </span>
        <span className="code-window__title">{title}</span>
        {copy && commands && <CopyButton text={commands} label="Copy" />}
      </div>
      <pre>
        <code>
          {lines.map((l, i) => {
            if (l.cmd !== undefined) {
              const cmd = l.comment ? l.cmd.padEnd(pad) : l.cmd;
              return (
                <span className="term__line" key={i}>
                  <span className="term__prompt">$ </span>
                  <span className="term__cmd">{cmd}</span>
                  {l.comment && <span className="term__comment">{`# ${l.comment}`}</span>}
                </span>
              );
            }
            const m = mark(l.out ?? '');
            const tone = ['term__out', l.tone && `term__out--${l.tone}`].filter(Boolean).join(' ');
            if (m) {
              const sym = `term__sym term__sym--${m.sym === '✓' ? 'ok' : m.sym === '✗' ? 'err' : 'warn'}`;
              return (
                <span className={`term__line ${tone}`} key={i}>
                  {m.lead}
                  <span className={sym}>{m.sym}</span>
                  {m.rest}
                </span>
              );
            }
            // Keep leading spaces outside the styled text (URLs are underlined).
            const [, lead = '', text = ''] = (l.out ?? '').match(/^(\s*)(.*)$/) ?? [];
            return (
              <span className="term__line" key={i}>
                {lead}
                <span className={tone}>{text || ' '}</span>
              </span>
            );
          })}
        </code>
      </pre>
    </div>
  );
}
