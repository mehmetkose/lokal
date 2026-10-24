// Copy-to-clipboard for any element with [data-copy]. The attribute holds the
// text to copy. One delegated listener covers every copy button: the React
// ones (CopyButton) and the docs code frames (made by plugins/rehype-lokal.ts).

const COPIED_MS = 1800;

let live: HTMLElement | null = null;
const announce = (message: string) => {
  if (!live) {
    live = document.createElement('div');
    live.className = 'visually-hidden';
    live.setAttribute('aria-live', 'polite');
    document.body.append(live);
  }
  live.textContent = '';
  // A fresh text node makes screen readers repeat the message on every copy.
  window.setTimeout(() => (live!.textContent = message), 30);
};

const fallbackCopy = (text: string) => {
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.position = 'fixed';
  area.style.opacity = '0';
  document.body.append(area);
  area.select();
  const ok = document.execCommand('copy');
  area.remove();
  return ok;
};

export const copyText = async (text: string) => {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return fallbackCopy(text);
  }
};

// Labels are React-rendered text: change the text node's value in place so
// React's reference to it stays valid.
const setLabel = (el: HTMLElement, text: string) => {
  if (el.firstChild && el.firstChild.nodeType === Node.TEXT_NODE) el.firstChild.nodeValue = text;
  else el.textContent = text;
};

const timers = new WeakMap<HTMLElement, number>();

const onClick = async (event: MouseEvent) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>('[data-copy]');
  if (!button) return;
  const text = button.dataset.copy ?? '';
  if (!text || !(await copyText(text))) return;

  const labels = button.querySelectorAll<HTMLElement>('[data-copy-label]');
  labels.forEach((el) => {
    el.dataset.idle ??= el.textContent ?? '';
    setLabel(el, 'Copied');
  });
  button.classList.add('is-copied');
  announce('Copied to clipboard');

  window.clearTimeout(timers.get(button));
  timers.set(
    button,
    window.setTimeout(() => {
      button.classList.remove('is-copied');
      labels.forEach((el) => setLabel(el, el.dataset.idle ?? ''));
    }, COPIED_MS),
  );
};

let installed = false;

/** Adds the document-wide click listener (once). */
export function installCopy() {
  if (installed) return;
  installed = true;
  document.addEventListener('click', onClick);
}
