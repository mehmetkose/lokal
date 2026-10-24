// Closing call to action: the orange box with the install command.
import { SectionTitle } from '../ui/SectionTitle';
import { InstallSnippet } from '../ui/InstallSnippet';
import { install, links, site } from '../../data/site';
import { withBase } from '../../lib/url';
import './InstallCta.css';

export function InstallCta() {
  return (
    <section className="section cta" id="install">
      <div className="container">
        <div className="cta__box">
          <div className="cta__text">
            <SectionTitle
              subtitle="get started"
              title="brew install. lokal setup. <span class='c-yellow'>Done.</span>"
              width={760}
              subtitleColor="white"
              titleColor="white"
            />
            <p className="text-body cta__lead" data-reveal="">
              Install with Homebrew, then run <code>{install.setup}</code> once. Every service after that takes one
              command and no password.
            </p>
          </div>

          <div className="cta__install" data-reveal="">
            <InstallSnippet variant="light" />
            <InstallSnippet variant="light" command={install.setup} />
          </div>

          <p className="cta__more" data-reveal="">
            {`Requires ${site.requires}. Or grab a `}
            <a href={links.releases}>prebuilt binary</a>, or <a href={withBase('/docs/installation#from-source')}>build from source</a>.
          </p>
        </div>
      </div>
    </section>
  );
}
