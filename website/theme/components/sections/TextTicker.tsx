// Yellow band with an endlessly scrolling row of words separated by padlocks.
import { Fragment } from 'react';
import { TornEdge } from '../ui/TornEdge';
import { LogoMark } from '../layout/LogoMark';
import './TextTicker.css';

type Props = {
  words?: string[];
};

const defaultWords = ['Real names', 'Valid HTTPS', 'No sudo', 'No restarts', 'Loopback only', 'HTTP/2', 'Hot reload'];

export function TextTicker({ words = defaultWords }: Props) {
  return (
    <section className="ticker bg-yellow" aria-label={words.join(', ')}>
      <TornEdge position="top" color="yellow" />
      <div className="ticker__viewport">
        <div className="ticker__track">
          {[0, 1].map((copy) => (
            <div className="ticker__group" aria-hidden={copy === 1 ? 'true' : undefined} key={copy}>
              {words.map((word, i) => (
                <Fragment key={i}>
                  <span className="h2">{word}</span>
                  <LogoMark color="green" className="ticker__mark" />
                </Fragment>
              ))}
            </div>
          ))}
        </div>
      </div>
      <TornEdge position="bottom" color="yellow" />
    </section>
  );
}
