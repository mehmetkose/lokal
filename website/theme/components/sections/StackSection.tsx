// "Works with your stack": green band with a slider of tool cards.
import { SectionTitle } from '../ui/SectionTitle';
import { TornEdge } from '../ui/TornEdge';
import { Slider } from '../ui/Slider';
import { StackCard } from '../cards/StackCard';
import { stack } from '../../data/product';
import './StackSection.css';

export function StackSection() {
  return (
    <section className="section section--sm stack bg-green" id="stack">
      <TornEdge position="top" color="green" />
      <div className="container stack__inner">
        <SectionTitle
          subtitle="works with your stack"
          title="Hot reload, WebSockets, HTTP/2. <span class='c-yellow'>It all passes through.</span>"
          width={820}
          subtitleColor="yellow"
          titleColor="white"
        />

        <Slider label="Tools that work with lokal">
          {stack.map((item) => (
            <StackCard key={item.title} title={item.title} text={item.text} code={item.code} href={item.href} />
          ))}
        </Slider>
      </div>
      <TornEdge position="bottom" color="green" />
    </section>
  );
}
