import { usePage, usePages } from '@rspress/core/runtime';
import { useMemo } from 'react';
import { type Doc, type DocData, getDocs } from './docs';

/** All published docs, in sidebar order. */
export function useDocs(): Doc[] {
  const { pages } = usePages();
  return useMemo(() => getDocs(pages), [pages]);
}

/** The current docs page: its URL, frontmatter and headings. */
export function useCurrentDoc() {
  const { page } = usePage();
  return {
    href: page.routePath,
    data: page.frontmatter as unknown as DocData,
    toc: page.toc,
  };
}
