// "Performance": bento grid of measured numbers from the live test, which
// count up when they scroll into view.
import { type CSSProperties, useEffect, useRef } from 'react';
import { SectionTitle } from '../ui/SectionTitle';
import { TerminalWindow } from '../ui/TerminalWindow';
import { Button } from '../ui/Button';
import { cx } from '../../lib/cx';
import { perfStats, perfLines, perfReport } from '../../data/product';
import './Performance.css';

const tones = ['green', 'yellow', 'dark', 'orange'] as const;
const stats = perfStats.map((s, i) => ({ ...s, tone: tones[i] }));
const [a, b, c, d] = stats;

export function Performance() {
  const ref = useRef<HTMLElement>(null);

  // Count each number up from 0 the first time it scrolls into view. The server
  // HTML holds the final values; the text is changed on the DOM, not through state.
  useEffect(() => {
    const counters = ref.current?.querySelectorAll<HTMLElement>('[data-count]');
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    if (!counters || reduced || !('IntersectionObserver' in window)) return;

    const frames = new Map<HTMLElement, number>();
    const run = (el: HTMLElement) => {
      const target = Number(el.dataset.count) || 0;
      const decimals = Number(el.dataset.decimals) || 0;
      const duration = 1600;
      const start = performance.now();
      const tick = (now: number) => {
        const t = Math.min(1, (now - start) / duration);
        const eased = 1 - Math.pow(1 - t, 3);
        el.textContent = (target * eased).toFixed(decimals);
        if (t < 1) frames.set(el, requestAnimationFrame(tick));
        else frames.delete(el);
      };
      frames.set(el, requestAnimationFrame(tick));
    };

    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          io.unobserve(entry.target);
          run(entry.target as HTMLElement);
        }
      },
      { threshold: 0.6 },
    );

    counters.forEach((el) => {
      el.textContent = (0).toFixed(Number(el.dataset.decimals) || 0);
      io.observe(el);
    });

    return () => {
      io.disconnect();
      frames.forEach((id) => cancelAnimationFrame(id));
    };
  }, []);

  return (
    <section className="perf" id="performance" ref={ref}>
      <div className="container perf__inner">
        <SectionTitle
          subtitle="performance"
          title="Fast enough that you <span class='c-orange'>never wait for it</span>"
          width={680}
          className="perf__title"
        />

        <div className="perf__grid">
          {[a, b, c, d].map((s, i) => (
            <div
              key={i}
              className={cx('fact-card', `fact-card--${s.tone}`, `perf__stat--${i + 1}`)}
              data-reveal=""
              style={{ '--reveal-delay': `${(i % 2) * 0.1}s` } as CSSProperties}
            >
              <div className="fact-card__body">
                <p className="fact-card__value">
                  <span data-count={s.value} data-decimals={s.decimals}>
                    {s.value.toFixed(s.decimals)}
                  </span>
                  <span className="fact-card__unit">{s.unit}</span>
                </p>
                <p className="fact-card__label">{s.label}</p>
                <p className="fact-card__target">{s.target}</p>
              </div>
            </div>
          ))}

          <div className="perf__report" data-reveal="" style={{ '--reveal-delay': '0.1s' } as CSSProperties}>
            <TerminalWindow
              lines={perfLines}
              title="live test · L23 Performance"
              copy={false}
              className="perf__terminal"
            />
          </div>

          <div className="perf__note" data-reveal="" style={{ '--reveal-delay': '0.15s' } as CSSProperties}>
            <p className="text-body">
              {`Measured by lokal's live test on macOS ${perfReport.macos} with lokal ${perfReport.lokal}, against the real background service. TLS is verified exactly the way macOS verifies it, and headless Chrome loads the pages. ${perfReport.checks}`}
            </p>
            <Button href="/docs/concepts/performance" variant="green">
              Targets & results
            </Button>
          </div>
        </div>
      </div>
      <div className="container perf__line" aria-hidden="true">
        <span></span>
      </div>
    </section>
  );
}
