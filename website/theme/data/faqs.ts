// Home page FAQ, from README.md "FAQ" and "Status". Keep in sync with
// site/docs/faq.md. `backticks` render as code.
export const faqs = [
  {
    question: 'How is this different from `mkcert`?',
    answer:
      'mkcert creates locally trusted certificates. You still have to configure DNS (or `/etc/hosts`) and a server or proxy for each name. lokal does names, certificates and routing together, and its CA is name-constrained, so it can only sign `.lokal` and `.local` names.',
  },
  {
    question: 'Does it work with Docker?',
    answer:
      "Yes, for ports you publish on your Mac. With `docker run -p 8080:80 …`, run `lokal service add app 8080`. Containers can't resolve lokal's names themselves.",
  },
  {
    question: 'Can I open my dev URLs on my phone or another computer?',
    answer: 'Not yet. lokal listens only on loopback by design. A LAN mode is on the roadmap.',
  },
  {
    question: 'Does it edit `/etc/hosts`?',
    answer: 'No. `lokal doctor` warns if `/etc/hosts` has an entry that conflicts with a lokal name.',
  },
  {
    question: 'Why only `.lokal` and `.local`, and not `.test` or `.internal`?',
    answer:
      'The CA is trusted system-wide, so it is constrained to exactly the TLDs lokal serves. Adding `.test` or `.internal` would let a leaked key sign names that companies use for real internal hosts.',
  },
  {
    question: 'Why does setup ask for my password twice?',
    answer:
      'Once is sudo in Terminal. The second time is a macOS dialog, because macOS asks separately before it changes certificate trust settings.',
  },
  {
    question: 'Is it macOS only?',
    answer:
      "Mostly: macOS 14 Sonoma or later, developed and tested on Apple Silicon; Intel builds are published but haven't been tested yet. Linux is a preview, with .deb and .rpm packages since 0.5.1, for systemd 258 or later (such as Ubuntu 26.04 or Fedora 43), with .lokal names only. Windows support can be added later.",
  },
  {
    question: 'Is lokal free?',
    answer: "Yes. It's open source, licensed under Apache-2.0.",
  },
];
