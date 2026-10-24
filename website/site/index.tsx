// The landing page (was web/astro/src/pages/index.astro).
import { useHead } from '@rspress/core/runtime';
import { Hero } from '../theme/components/home/Hero';
import { CodingAgents } from '../theme/components/sections/CodingAgents';
import { FaqSection } from '../theme/components/sections/FaqSection';
import { Features } from '../theme/components/sections/Features';
import { HowItWorks } from '../theme/components/sections/HowItWorks';
import { InstallCta } from '../theme/components/sections/InstallCta';
import { Performance } from '../theme/components/sections/Performance';
import { StackSection } from '../theme/components/sections/StackSection';
import { TextTicker } from '../theme/components/sections/TextTicker';
import { SITE_URL, links, site } from '../theme/data/site';

export const frontmatter = { pageType: 'home' };

// Structured data for search engines.
const jsonLd = {
  '@context': 'https://schema.org',
  '@type': 'SoftwareApplication',
  name: site.name,
  description: site.tagline,
  url: SITE_URL,
  applicationCategory: 'DeveloperApplication',
  operatingSystem: 'macOS 14 or later',
  softwareVersion: site.version,
  license: links.license,
  codeRepository: links.repo,
  downloadUrl: links.releases,
  author: { '@type': 'Person', name: site.author },
  offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
};

export default function HomePage() {
  useHead({ script: [{ type: 'application/ld+json', innerHTML: JSON.stringify(jsonLd) }] });
  return (
    <>
      <Hero />
      <Features />
      <TextTicker />
      <HowItWorks />
      <StackSection />
      <Performance />
      <CodingAgents />
      <FaqSection />
      <InstallCta />
    </>
  );
}
