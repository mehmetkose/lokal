// One line of a TerminalWindow session.
export interface TerminalLine {
  /** A command, shown after a `$ ` prompt. */
  cmd?: string;
  /** Trailing `# comment` after the command (hidden on phones). */
  comment?: string;
  /** An output line. */
  out?: string;
  tone?: 'ok' | 'url' | 'muted' | 'warn';
}
