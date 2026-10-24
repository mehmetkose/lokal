// Build-time version of what web/astro/src/scripts/docs.ts did in the browser,
// plus a few fixes that make Rspress's Markdown output match the Astro site:
//
// 1. Code blocks get the `.code-window` frame: dots, a language label and a copy
//    chip. Terminal sessions (`console`) copy only their `$ ` commands.
// 2. Code in table cells may wrap between words only; long cells get `is-long`.
// 3. Heading anchors: Rspress puts `a.rp-header-anchor` first in every heading.
//    The design appends `a.heading-anchor` to h2/h3 instead.
// 4. Internal links: Rspress rewrites them to `/docs/x.html`. The site uses clean URLs, and
//    lives under the base path (/lokal/), which Rspress doesn't add to Markdown links.
// 5. External links: Rspress adds target="_blank" rel="noopener noreferrer". The
//    Astro site didn't.
import type { Element, ElementContent, Root } from 'hast';
import { fromHtml } from 'hast-util-from-html';
import { toString } from 'hast-util-to-string';
import { SKIP, visit } from 'unist-util-visit';
import { withBase } from '../theme/lib/url';

const COPY_ICON =
  '<svg class="copy-btn__icon copy-btn__icon--idle" width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></g></svg>';
const CHECK_ICON =
  '<svg class="copy-btn__icon copy-btn__icon--done" width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';

const LANG_LABELS: Record<string, string> = {
  console: 'terminal',
  shellsession: 'terminal',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  plaintext: 'text',
};

const escapeHtml = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

// Shiki's output uses a `class` string; hast from Markdown uses a `className` list.
const classList = (node: Element): string[] => {
  const value: unknown = node.properties.className ?? node.properties.class;
  return Array.isArray(value) ? value.map(String) : typeof value === 'string' ? value.split(/\s+/) : [];
};

const addClass = (node: Element, name: string) => {
  node.properties.className = [...classList(node), name];
  delete node.properties.class;
};

// Terminal sessions copy only the commands, without their `$ ` prompt.
const copyTextFor = (pre: Element, lang: string) => {
  const text = toString(pre).replace(/\n$/, '');
  if (lang !== 'console' && lang !== 'shellsession') return text;
  const commands = text
    .split('\n')
    .filter((line) => line.startsWith('$ '))
    .map((line) => line.slice(2).replace(/\s+#\s.*$/, ''));
  return commands.length ? commands.join('\n') : text;
};

const codeFrame = (pre: Element): Element => {
  const lang = String(pre.properties.lang ?? '') || 'text';
  const title = LANG_LABELS[lang] ?? lang;
  const copy = copyTextFor(pre, lang);

  // The <pre> as Astro's Shiki integration rendered it. Rspress's extra
  // properties (lang, title, wrapCode, …) would otherwise become attributes.
  pre.properties = {
    className: ['astro-code', 'css-variables'],
    style: 'background-color:var(--astro-code-background);color:var(--astro-code-foreground); overflow-x: auto;',
    tabIndex: 0,
    dataLanguage: lang,
  };

  const frame = fromHtml(
    '<div class="code-window code-window--docs"><div class="code-window__bar">' +
      '<span class="code-window__dots" aria-hidden="true"><span></span><span></span><span></span></span>' +
      `<span class="code-window__title">${escapeHtml(title)}</span>` +
      `<button type="button" class="copy-btn copy-btn--chip" data-copy="${escapeHtml(copy)}" aria-label="Copy code">` +
      `<span class="copy-btn__face">${COPY_ICON}${CHECK_ICON}<span data-copy-label>Copy</span></span></button>` +
      '</div></div>',
    { fragment: true },
  ).children[0] as Element;
  frame.children.push(pre);
  return frame;
};

// Cells keep code on one line. Code with spaces may wrap, but only between words.
const splitCodeWords = (code: Element) => {
  const parts = toString(code).split(/(\s+)/);
  if (parts.length < 3) return;
  code.children = parts
    .filter((part) => part !== '')
    .map((part): ElementContent =>
      /^\s*$/.test(part)
        ? { type: 'text', value: part }
        : { type: 'element', tagName: 'span', properties: {}, children: [{ type: 'text', value: part }] },
    );
  addClass(code, 'code-words');
};

const cleanInternalHref = (href: string) =>
  href.replace(/\/index\.html(?=[#?]|$)/, '/').replace(/\.html(?=[#?]|$)/, '');

export function rehypeLokal() {
  return (tree: Root) => {
    visit(tree, 'element', (node, index, parent) => {
      // 1. Code frames
      if (node.tagName === 'pre' && classList(node).includes('shiki') && parent && index !== undefined) {
        parent.children[index] = codeFrame(node);
        return SKIP;
      }

      // 2. Code in table cells; cells holding sentences get a min width on phones.
      if (node.tagName === 'td') {
        visit(node, 'element', (child) => {
          if (child.tagName === 'code') splitCodeWords(child);
        });
        if (toString(node).trim().length > 60) addClass(node, 'is-long');
        return;
      }

      // 3. Heading anchors
      if (/^h[1-6]$/.test(node.tagName)) {
        node.children = node.children.filter(
          (child) => !(child.type === 'element' && child.tagName === 'a' && classList(child).includes('rp-header-anchor')),
        );
        const id = node.properties.id;
        if ((node.tagName === 'h2' || node.tagName === 'h3') && id) {
          node.children.push({
            type: 'element',
            tagName: 'a',
            properties: { className: ['heading-anchor'], href: `#${id}`, ariaLabel: `Link to "${toString(node)}"` },
            children: [{ type: 'text', value: '#' }],
          });
        }
        return;
      }

      // 4 and 5. Links
      if (node.tagName === 'a' && typeof node.properties.href === 'string') {
        const href = node.properties.href;
        if (href.startsWith('/') && !href.startsWith('//')) {
          node.properties.href = withBase(cleanInternalHref(href));
        } else if (/^https?:\/\//.test(href) && node.properties.target === '_blank') {
          delete node.properties.target;
          delete node.properties.rel;
        }
      }
    });
  };
}
