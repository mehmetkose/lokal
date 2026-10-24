// "Why lokal": eight feature cards with icon seals (README.md, "Why lokal").
import type { CSSProperties } from 'react';
import { SectionTitle } from '../ui/SectionTitle';
import { Icon } from '../ui/Icon';
import { features } from '../../data/product';
import { inlineCode } from '../../lib/text';
import './Features.css';

type Props = {
  divider?: boolean;
};

// Scalloped "seal" shape for the icon badges.
const seal = Array.from({ length: 40 }, (_, i) => {
  const a = (Math.PI * 2 * i) / 40;
  const r = i % 2 === 0 ? 50 : 45;
  return `${(50 + r * Math.cos(a)).toFixed(2)}% ${(50 + r * Math.sin(a)).toFixed(2)}%`;
}).join(', ');

export function Features({ divider = false }: Props) {
  return (
    <section className="section choose" id="features" style={{ '--seal': `polygon(${seal})` } as CSSProperties}>
      <div className="container choose__inner">
        <SectionTitle
          subtitle="why lokal"
          title="Your dev servers, <span class='c-orange'>behaving like production</span>"
          width={760}
        />
        <div className="choose__grid">
          {features.map((item, i) => (
            <article
              className="choose-card"
              data-reveal=""
              style={{ '--reveal-delay': `${(i % 4) * 0.08}s` } as CSSProperties}
              key={item.title}
            >
              <span className="choose-card__badge">
                <Icon name={item.icon} size={52} className="choose-card__icon" />
              </span>
              <div className="choose-card__text">
                <h3 className="h5">{item.title}</h3>
                <p className="text-body with-code" dangerouslySetInnerHTML={{ __html: inlineCode(item.text) }} />
              </div>
            </article>
          ))}
        </div>
        {divider && <hr className="choose__line" />}
      </div>
    </section>
  );
}
