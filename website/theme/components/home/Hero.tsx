// Home hero: headline, install command and the README session in a tilted
// terminal, with a browser bar showing the result.
import type { CSSProperties } from 'react';
import { Button } from '../ui/Button';
import { TornEdge } from '../ui/TornEdge';
import { Icon } from '../ui/Icon';
import { InstallSnippet } from '../ui/InstallSnippet';
import { TerminalWindow } from '../ui/TerminalWindow';
import { BrowserBar } from '../ui/BrowserBar';
import { heroSession } from '../../data/product';
import { install, links } from '../../data/site';
import './Hero.css';

const cards = [
  { icon: 'laptop', label: 'macOS 14+ · Linux' },
  { icon: 'shield', label: 'Loopback only' },
  { icon: 'zap', label: 'No sudo after setup' },
] as const;

// One project, many services: each gets its own name. Listed back to front;
// the last one sits right behind the terminal and matches its session.
const urls = ['docs', 'web', 'console', 'backoffice', 'api'].map((service) => `https://${service}.myapp.lokal/`);

export function Hero() {
  return (
    <section className="hero bg-green">
      <div className="container hero__inner">
        <div className="hero__content">
          <div className="hero__top" data-reveal="">
            <p className="subtitle c-yellow">
              <span aria-hidden="true">// </span>open source · macOS · Linux
            </p>
            <h1 className="h1 c-white">
              Real <span className="c-yellow">HTTPS domains</span> for your local dev servers
            </h1>
            <p className="hero__lead">
              <code>https://api.myapp.lokal</code> instead of <code>http://localhost:4000</code>, with a certificate
              your browser, curl, Node and Python trust. Set it up once. After that, adding a service takes
              milliseconds and never asks for a password.
            </p>

            <div className="hero__actions">
              <InstallSnippet className="hero__install" />
              <div className="hero__row">
                <Button href="/docs/quick-start" variant="orange">
                  Read the docs
                </Button>
                <p className="hero__note">
                  Then run <code>{install.setup}</code> once.
                  <br />
                  <a href={links.repo}>
                    <Icon name="github" size={16} /> Star mehmetkose/lokal
                  </a>
                </p>
              </div>
            </div>
          </div>

          <ul className="hero__cards" data-reveal="" style={{ '--reveal-delay': '0.15s' } as CSSProperties}>
            {cards.map((card) => (
              <li className="hero-card" key={card.label}>
                <Icon name={card.icon} size={30} className="hero-card__icon" />
                <span className="text-label c-white">{card.label}</span>
              </li>
            ))}
          </ul>
        </div>

        <div className="hero__visual" data-reveal="" style={{ '--reveal-delay': '0.1s' } as CSSProperties}>
          <div className="hero__browsers">
            {urls.map((url) => (
              <BrowserBar url={url} className="hero__browser" key={url} />
            ))}
          </div>
          <TerminalWindow lines={heroSession} title="~/code/myapp — zsh" tilt={3} className="hero__terminal" />
        </div>
      </div>
      <TornEdge position="bottom" color="green" />
    </section>
  );
}
