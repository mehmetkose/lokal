// Source for the social card (public/og.png, 1200×630) and the home-screen
// icon (public/apple-touch-icon.png, 180×180). Not linked anywhere; to
// regenerate, screenshot #og-card and #touch-icon at device pixel ratio 1.
// Renders the <body> content only; site/brand.tsx sets the head tags and the
// `brand` class on <body>.
import { Logo } from '../components/layout/Logo';
import { LogoMark } from '../components/layout/LogoMark';
import { TerminalWindow } from '../components/ui/TerminalWindow';
import { heroSession } from '../data/product';
import { install, site } from '../data/site';
import './Brand.css';

export function Brand() {
  return (
    <>
      <div className="og" id="og-card">
        <div className="og__text">
          <Logo size={58} isStatic />
          <h1 className="h1 c-white og__title">
            Real <span className="c-yellow">HTTPS domains</span> for your local dev servers
          </h1>
          <p className="og__cmd">
            <span>$</span>
            {` ${install.brew}`}
          </p>
        </div>
        <div className="og__visual">
          <TerminalWindow lines={heroSession.slice(1, 5)} title="~/code/myapp — zsh" tilt={4} copy={false} />
        </div>
        <p className="og__foot">{`mehmetkose.github.io/lokal · open source · ${site.requires}`}</p>
      </div>

      <div className="touch" id="touch-icon">
        <LogoMark size={120} />
      </div>
    </>
  );
}
