// Accordion: one item open at a time. Open = orange, closed = green.
import { useEffect, useRef } from 'react';
import { inlineCode } from '../../lib/text';
import { Icon } from './Icon';
import './Faq.css';

type Props = {
  /** Answers may mark code with `backticks`. */
  items: { question: string; answer: string }[];
  /** Index of the item open on load (-1 for none). */
  open?: number;
};

export function Faq({ items, open = 0 }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  // Close the other items when one opens. `open` is left to the browser after
  // the first render, so React never fights the user's clicks.
  useEffect(() => {
    const details = [...(ref.current?.querySelectorAll<HTMLDetailsElement>('details') ?? [])];
    const handlers = details.map((item) => {
      const onToggle = () => {
        if (item.open) details.forEach((other) => other !== item && (other.open = false));
      };
      item.addEventListener('toggle', onToggle);
      return () => item.removeEventListener('toggle', onToggle);
    });
    return () => handlers.forEach((off) => off());
  }, []);

  return (
    <div className="faq" data-faq="" ref={ref}>
      {items.map((item, i) => (
        <details className="faq__item" open={i === open} data-reveal="" key={item.question}>
          <summary className="faq__question">
            <h3 className="h5 c-white" dangerouslySetInnerHTML={{ __html: inlineCode(item.question) }} />
            <span className="faq__icon">
              <Icon name="plus" size={18} className="faq__plus" />
              <Icon name="minus" size={18} className="faq__minus" />
            </span>
          </summary>
          <div className="faq__answer">
            <p
              className="text-body c-white with-code with-code--on-dark"
              dangerouslySetInnerHTML={{ __html: inlineCode(item.answer) }}
            />
          </div>
        </details>
      ))}
    </div>
  );
}
