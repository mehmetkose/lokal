// "Built for coding agents": yellow band with the README's AGENTS.md snippet
// and the exit codes (README.md "Using lokal with coding agents", SPEC §4.9).
import type { CSSProperties } from 'react';
import { SectionTitle } from '../ui/SectionTitle';
import { TornEdge } from '../ui/TornEdge';
import { Button } from '../ui/Button';
import { Icon, type IconName } from '../ui/Icon';
import { CodeBlock } from '../ui/CodeBlock';
import { agentsSnippet, exitCodes } from '../../data/product';
import { links } from '../../data/site';
import { inlineCode } from '../../lib/text';
import './CodingAgents.css';

const points: readonly { icon: IconName; text: string }[] = [
  { icon: 'refresh', text: 'Every command is safe to re-run, so an agent can repeat a step without breaking anything.' },
  { icon: 'braces', text: '`lokal list --json` and `lokal status --json` are a stable contract (`"schema": 1`).' },
  { icon: 'terminal', text: "`lokal --help` has a section written for agents, so they can learn the workflow from the binary alone." },
  { icon: 'lock', text: 'Agents never run `lokal setup` or sudo. If lokal isn’t set up, they ask you.' },
];

export function CodingAgents() {
  return (
    <section className="section section--sm agents bg-yellow" id="agents">
      <TornEdge position="top" color="yellow" />
      <div className="container agents__inner">
        <div className="agents__text">
          <SectionTitle
            subtitle="for coding agents"
            title="Your agent can drive it, <span class='c-orange'>you keep the password</span>"
            align="left"
            width={560}
            subtitleColor="green"
          />
          <ul className="agents__points">
            {points.map((p, i) => (
              <li
                key={p.icon}
                className="agents__point"
                data-reveal=""
                style={{ '--reveal-delay': `${i * 0.06}s` } as CSSProperties}
              >
                <span className="agents__icon">
                  <Icon name={p.icon} size={22} />
                </span>
                <p className="text-body with-code" dangerouslySetInnerHTML={{ __html: inlineCode(p.text) }} />
              </li>
            ))}
          </ul>
          <p className="agents__history text-body" data-reveal="">
            lokal v0.1 was itself built by a coding agent working from the spec, with human sign-off for every
            privileged step. <a href={`${links.repo}/blob/main/docs/history/README.md`}>Read the build log</a>.
          </p>
          <div data-reveal="">
            <Button href="/docs/guides/coding-agents" variant="green">
              Using lokal with agents
            </Button>
          </div>
        </div>

        <div className="agents__side">
          <div data-reveal="">
            <p className="agents__label">Add this to your repo's AGENTS.md or CLAUDE.md:</p>
            <CodeBlock code={agentsSnippet} lang="markdown" filename="AGENTS.md" className="agents__code" />
          </div>
          <div className="agents__exit" data-reveal="" style={{ '--reveal-delay': '0.1s' } as CSSProperties}>
            <h3 className="h6">Exit codes say what went wrong</h3>
            <dl className="agents__codes">
              {exitCodes.map((e) => (
                <div key={e.code}>
                  <dt>{e.code}</dt>
                  <dd>{e.meaning}</dd>
                </div>
              ))}
            </dl>
          </div>
        </div>
      </div>
      <TornEdge position="bottom" color="yellow" />
    </section>
  );
}
