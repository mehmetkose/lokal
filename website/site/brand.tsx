// Source for the social card (public/og.png, 1200×630) and the home-screen
// icon (public/apple-touch-icon.png, 180×180). Not linked anywhere; to
// regenerate, screenshot #og-card and #touch-icon at device pixel ratio 1.
import { useHead } from '@rspress/core/runtime';
import { Brand } from '../theme/pages/Brand';

export const frontmatter = { pageType: 'blank' };

export default function BrandPage() {
  useHead({
    htmlAttrs: { lang: 'en' },
    title: 'lokal brand assets',
    meta: [{ name: 'robots', content: 'noindex' }],
    bodyAttrs: { class: 'brand' },
  });
  return <Brand />;
}
