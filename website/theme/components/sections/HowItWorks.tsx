// "How it works": three numbered steps and the README usage session.
import type { CSSProperties } from 'react';
import { SectionTitle } from '../ui/SectionTitle';
import { Icon } from '../ui/Icon';
import { Button } from '../ui/Button';
import { TerminalWindow } from '../ui/TerminalWindow';
import { steps, usageSession } from '../../data/product';
import { inlineCode } from '../../lib/text';
import './HowItWorks.css';

export function HowItWorks() {
  return (
    <section className="section how" id="how-it-works">
      <div className="container how__inner">
        <SectionTitle
          subtitle="how it works"
          title="Set up once. <span class='c-orange'>Then every service is one command.</span>"
          width={780}
        />

        <ol className="how__steps">
          {steps.map((step, i) => (
            <li
              className="how-card"
              data-reveal=""
              style={{ '--reveal-delay': `${i * 0.1}s` } as CSSProperties}
              key={step.title}
            >
              <div className="how-card__top">
                <span className="h3 how-card__num">{String(i + 1).padStart(2, '0')}</span>
                <span className="how-card__icon">
                  <Icon name={step.icon} size={28} />
                </span>
              </div>
              <div className="how-card__text">
                <h3 className="h5">{step.title}</h3>
                <p className="text-body with-code" dangerouslySetInnerHTML={{ __html: inlineCode(step.text) }} />
              </div>
              <code className="how-card__cmd">
                <span aria-hidden="true">{'$ '}</span>
                {step.cmd}
              </code>
            </li>
          ))}
        </ol>

        <div className="how__session" data-reveal="">
          <TerminalWindow lines={usageSession} title="~/code/shop — zsh" />
        </div>

        <div className="how__cta" data-reveal="">
          <Button href="/docs/concepts/how-it-works" variant="green">
            How lokal works
          </Button>
          <Button href="/docs/quick-start" variant="yellow">
            Quick start
          </Button>
        </div>
      </div>
    </section>
  );
}
