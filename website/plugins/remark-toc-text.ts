// Rspress builds each page's table of contents before our remark plugins run,
// so its text still has Markdown backticks and straight quotes. Astro's TOC
// showed the heading's plain text after smart punctuation. This rewrites the
// TOC text the same way; it must run after remark-smartypants.
import type { Heading, Root } from 'mdast';
import { toString } from 'mdast-util-to-string';
import type { Processor } from 'unified';

type TocItem = { id: string; text: string; depth: number };

export function remarkTocText(this: Processor) {
  const data = this.data() as { pageMeta?: { toc?: TocItem[] } };
  return (tree: Root, file: { path?: string }) => {
    const toc = data.pageMeta?.toc;
    if (!toc?.length) return;
    // Rspress's TOC lists the top-level h2–h4 headings, in order.
    const headings = tree.children.filter(
      (node): node is Heading => node.type === 'heading' && node.depth >= 2 && node.depth <= 4,
    );
    if (headings.length !== toc.length) {
      throw new Error(`remark-toc-text: ${toc.length} TOC items but ${headings.length} headings in ${file.path}`);
    }
    headings.forEach((heading, i) => {
      toc[i].text = toString(heading).trim();
    });
  };
}
