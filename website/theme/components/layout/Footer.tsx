import { footerNav, links, site } from '../../data/site';
import { Icon } from '../ui/Icon';
import { TornEdge } from '../ui/TornEdge';
import { Logo } from './Logo';
import { withBase } from '../../lib/url';
import './Footer.css';

export function Footer() {
  const year = new Date().getFullYear();

  return (
    <footer className="footer bg-green">
      <TornEdge position="top" color="green" />
      <div className="container footer__inner">
        <div className="footer__cols">
          <div className="footer__col footer__col--about">
            <Logo size={30} />
            <p className="footer__text">{`${site.tagline} Written in Rust, for ${site.requires}.`}</p>
            <div className="footer__socials">
              <a href={links.repo} target="_blank" rel="noopener" aria-label="lokal on GitHub" className="footer__social">
                <Icon name="github" size={22} />
              </a>
              <a href={links.releases} target="_blank" rel="noopener" aria-label="Releases" className="footer__social">
                <Icon name="download" size={22} />
              </a>
            </div>
          </div>

          {footerNav.map((col) => (
            <div className="footer__col" key={col.label}>
              <h2 className="h6 c-white">{col.label}</h2>
              <ul className="footer__list">
                {col.items.map((item) => (
                  <li key={item.href}>
                    <a
                      href={withBase(item.href)}
                      className="footer__text footer__link"
                      target={item.external ? '_blank' : undefined}
                      rel={item.external ? 'noopener' : undefined}
                    >
                      {item.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>

        <div className="footer__bottom">
          <svg className="footer__wordmark" viewBox="0 0 1300 330" aria-hidden="true">
            <defs>
              <linearGradient id="wordmark-fade" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor="#fff" stopOpacity="0.9"></stop>
                <stop offset="1" stopColor="#fff" stopOpacity="0"></stop>
              </linearGradient>
            </defs>
            <text
              x="0"
              y="300"
              textLength="1300"
              lengthAdjust="spacingAndGlyphs"
              fill="none"
              stroke="url(#wordmark-fade)"
              strokeWidth="1"
            >
              lokal
            </text>
          </svg>
          <div className="footer__copy">
            {/* The year is fixed at build time; a client in a later year keeps the prerendered text. */}
            <p className="footer__text" suppressHydrationWarning>
              {`© ${year} ${site.author} · `}
              <a href={links.license} className="footer__link">
                {site.license}
              </a>
              {' · '}
              <a href={links.repo} className="footer__link">
                github.com/mehmetkose/lokal
              </a>
            </p>
          </div>
        </div>
      </div>
    </footer>
  );
}
