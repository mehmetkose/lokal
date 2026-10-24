import { useEffect } from 'react';

// Fade/slide [data-reveal] elements in as they enter the viewport. The `js`
// class on <html> (set by an inline script in the head) hides them until then.
// Classes are added directly, so React re-renders leave them alone.
export function useReveal(routePath: string) {
  useEffect(() => {
    const items = document.querySelectorAll<HTMLElement>('[data-reveal]:not(.is-visible)');
    if (!('IntersectionObserver' in window)) {
      items.forEach((el) => el.classList.add('is-visible'));
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add('is-visible');
            io.unobserve(entry.target);
          }
        }
      },
      { rootMargin: '0px 0px -60px 0px', threshold: 0.05 },
    );
    items.forEach((el) => io.observe(el));
    return () => io.disconnect();
  }, [routePath]);
}
