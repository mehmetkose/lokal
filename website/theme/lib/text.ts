// Escapes text for HTML and turns `backticks` into <code> elements, so data
// files can mark commands and flags without writing HTML.
const escape = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

export const inlineCode = (text: string) =>
  escape(text).replace(/`([^`]+)`/g, '<code>$1</code>');
