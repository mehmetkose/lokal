// <head> tags for a page: title, description, canonical URL and social cards.
// Same tags as web/astro/src/layouts/Layout.astro; the ones every page shares
// (icons, fonts, theme colour) are in rspress.config.ts.
import { useHead, usePage } from '@rspress/core/runtime';
import { site } from '../../data/site';
import { absoluteUrl } from '../../lib/url';

type Props = {
  title?: string;
  description?: string;
  /** Open Graph type: the home page is a website, docs pages are articles. */
  ogType?: 'website' | 'article';
  /** Social card image, relative to the site root. */
  image?: string;
  noindex?: boolean;
};

export function Seo({ title, description = site.description, ogType = 'website', image = '/og.png', noindex = false }: Props) {
  const { page } = usePage();
  const pageTitle = title ? `${title} — ${site.name}` : site.title;
  const canonical = absoluteUrl(page.routePath);
  const imageUrl = absoluteUrl(image);
  const cardTitle = title ?? site.title;

  useHead({
    htmlAttrs: { lang: 'en' },
    title: pageTitle,
    meta: [
      { name: 'description', content: description },
      ...(noindex ? [{ name: 'robots', content: 'noindex' }] : []),
      { property: 'og:site_name', content: site.name },
      { property: 'og:type', content: ogType },
      { property: 'og:title', content: cardTitle },
      { property: 'og:description', content: description },
      { property: 'og:url', content: canonical },
      { property: 'og:image', content: imageUrl },
      { property: 'og:image:width', content: '1200' },
      { property: 'og:image:height', content: '630' },
      { property: 'og:image:alt', content: `${site.name}: ${site.tagline}` },
      { name: 'twitter:card', content: 'summary_large_image' },
      { name: 'twitter:title', content: cardTitle },
      { name: 'twitter:description', content: description },
      { name: 'twitter:image', content: imageUrl },
    ],
    link: noindex ? [] : [{ rel: 'canonical', href: canonical }],
  });

  return null;
}
