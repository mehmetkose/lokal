// Code highlighted with Shiki at build time (see `highlight` in rspress.config.ts),
// keyed by the trimmed source. Holds the inner HTML of Shiki's <code> element.
declare const __LOKAL_HIGHLIGHTED__: Record<string, string>;

export function highlighted(code: string): string {
  const html = __LOKAL_HIGHLIGHTED__[code];
  if (html === undefined) {
    throw new Error(`Code block not highlighted at build time. Add it to \`highlight\` in rspress.config.ts:\n${code}`);
  }
  return html;
}
