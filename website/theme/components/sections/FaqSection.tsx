import { SectionTitle } from '../ui/SectionTitle';
import { Faq } from '../ui/Faq';
import { Button } from '../ui/Button';
import { faqs } from '../../data/faqs';
import './FaqSection.css';

type Props = {
  showTitle?: boolean;
};

export function FaqSection({ showTitle = true }: Props) {
  return (
    <section className="section faq-section" id="faq">
      <div className="container faq-section__inner">
        {showTitle && <SectionTitle subtitle="faq" title="Questions, answered" width={740} />}
        <Faq items={faqs} />
        <Button href="/docs/faq" variant="green">
          More answers in the docs
        </Button>
      </div>
    </section>
  );
}
