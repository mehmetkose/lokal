// A white card for the "works with your stack" slider: a tool, a sentence and
// the one command that makes it work.
import { CopyButton } from '../ui/CopyButton';
import { Icon } from '../ui/Icon';
import { inlineCode } from '../../lib/text';
import { withBase } from '../../lib/url';
import './StackCard.css';

type Props = {
  title: string;
  text: string;
  code: string;
  href: string;
};

export function StackCard({ title, text, code, href }: Props) {
  return (
    <article className="stack-card" data-reveal="">
      <div className="stack-card__info">
        <h3 className="h5">{title}</h3>
        <p className="text-body with-code" dangerouslySetInnerHTML={{ __html: inlineCode(text) }} />
      </div>
      <div className="stack-card__footer">
        <div className="stack-card__code">
          <code>
            <span className="stack-card__prompt" aria-hidden="true">
              {'$ '}
            </span>
            {code}
          </code>
          <CopyButton text={code} />
        </div>
        <a href={withBase(href)} className="stack-card__link">
          Read the guide <Icon name="arrow-right" size={16} />
        </a>
      </div>
    </article>
  );
}
