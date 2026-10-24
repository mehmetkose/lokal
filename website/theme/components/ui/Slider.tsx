// Horizontal card slider (scroll-snap) with prev/next arrow buttons below.
// Shows 3 cards on desktop, 2 on tablet, 1 on phones.
import { type ReactNode, useEffect, useRef } from 'react';
import { cx } from '../../lib/cx';
import { Icon } from './Icon';
import './Slider.css';

type Props = { label: string; className?: string; children: ReactNode };

export function Slider({ label, className, children }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const prevRef = useRef<HTMLButtonElement>(null);
  const nextRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const track = trackRef.current;
    const prev = prevRef.current;
    const next = nextRef.current;
    if (!track || !prev || !next) return;

    const step = () => {
      const first = track.firstElementChild as HTMLElement | null;
      return first ? first.offsetWidth + 20 : track.clientWidth;
    };
    // The buttons' disabled state is set directly: React never renders it.
    const update = () => {
      const max = track.scrollWidth - track.clientWidth - 2;
      prev.disabled = track.scrollLeft <= 2;
      next.disabled = track.scrollLeft >= max;
    };
    const onPrev = () => track.scrollBy({ left: -step() });
    const onNext = () => track.scrollBy({ left: step() });

    prev.addEventListener('click', onPrev);
    next.addEventListener('click', onNext);
    track.addEventListener('scroll', update, { passive: true });
    window.addEventListener('resize', update);
    update();
    return () => {
      prev.removeEventListener('click', onPrev);
      next.removeEventListener('click', onNext);
      track.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
    };
  }, []);

  return (
    <div className={cx('slider', className)} data-slider="">
      <div className="slider__track" data-slider-track="" role="region" aria-label={label} tabIndex={0} ref={trackRef}>
        {children}
      </div>
      <div className="slider__nav">
        <button className="arrow-btn" data-slider-prev="" aria-label="Previous" ref={prevRef}>
          <Icon name="chevron-left" />
        </button>
        <button className="arrow-btn" data-slider-next="" aria-label="Next" ref={nextRef}>
          <Icon name="chevron-right" />
        </button>
      </div>
    </div>
  );
}
