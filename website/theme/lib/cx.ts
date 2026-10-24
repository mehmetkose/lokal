// Joins class names, skipping falsy values (the React version of Astro's class:list).
export const cx = (...names: (string | false | null | undefined)[]) => names.filter(Boolean).join(' ') || undefined;
