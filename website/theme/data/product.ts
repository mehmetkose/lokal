// Product copy for the home page. Everything here comes from the lokal repo;
// keep it in sync when the README changes. Each block names its source.
import type { TerminalLine } from '../components/ui/terminal';
import type { IconName } from '../components/ui/icons';

// README.md, the opening console block. The setup comment is shorter than the
// README's so the longest line fits the hero terminal without scrolling.
export const heroSession: TerminalLine[] = [
  { cmd: 'lokal setup', comment: 'once; asks for your password' },
  { cmd: 'lokal project create myapp', comment: 'claims myapp.lokal' },
  { cmd: 'lokal service add api 4000' },
  { out: '✓ api.myapp.lokal → localhost:4000  (ready in 8ms)', tone: 'ok' },
  { out: '  https://api.myapp.lokal', tone: 'url' },
  { cmd: 'curl https://api.myapp.lokal', comment: 'works: no -k, no --cacert' },
];

// README.md, "Why lokal". The eighth card fills the 4×2 grid.
export const features: { icon: IconName; title: string; text: string }[] = [
  {
    icon: 'globe',
    title: 'Real names',
    text: 'Every service gets its own hostname, so cookies, CORS, OAuth redirect URLs and subdomain routing behave the way they do in production.',
  },
  {
    icon: 'lock',
    title: 'Valid HTTPS',
    text: 'Secure cookies, HTTP/2 and mixed-content rules work like they do in production, without `-k`, `--insecure` or clicking through browser warnings.',
  },
  {
    icon: 'zap',
    title: 'Fast and quiet',
    text: 'Adding, changing or removing a service needs no sudo, no prompts and no restarts.',
  },
  {
    icon: 'laptop',
    title: 'Nothing leaves your Mac',
    text: 'Every listener is on loopback, and names are never announced on your network.',
  },
  {
    icon: 'shield',
    title: 'A narrowly scoped CA',
    text: "lokal's CA can only sign `.lokal` and `.local` names, never public domains or IP addresses. Even a leaked key can't impersonate a real website.",
  },
  {
    icon: 'layers',
    title: 'Your whole dev stack',
    text: 'WebSockets (Vite, Next.js and webpack hot reload), server-sent events, long polling, streaming uploads and HTTP/2 all pass through.',
  },
  {
    icon: 'bot',
    title: 'Scriptable',
    text: 'Every command is safe to re-run, every list command has a stable `--json` output, and exit codes say what went wrong.',
  },
  {
    icon: 'github',
    title: 'Open source',
    text: 'Written in Rust and licensed under Apache-2.0. Read the code, the spec and how v0.1 was built.',
  },
];

// README.md, "Setup" and "How it works".
export const steps: { icon: IconName; title: string; text: string; cmd: string }[] = [
  {
    icon: 'shield',
    title: 'Set up once',
    text: "Creates a CA that can only sign `.lokal` and `.local` names, trusts it in the System keychain, points `.lokal` lookups at lokal's DNS server and installs a background service that runs as you. Apart from uninstalling, it's the only time lokal asks for your password.",
    cmd: 'lokal setup',
  },
  {
    icon: 'zap',
    title: 'Add a service',
    text: 'A project owns one domain, and each service is a subdomain routed to a port on your Mac. No sudo, no prompts, no restarts. When the command returns, the URL already works.',
    cmd: 'lokal service add api 4000',
  },
  {
    icon: 'globe',
    title: 'Open it',
    text: 'lokal answers `*.lokal` lookups with loopback, issues each name its own certificate in memory and routes by `Host` and SNI to `localhost:<port>`.',
    cmd: 'lokal open api',
  },
];

// README.md, "Usage" (verbatim, without the .local line).
export const usageSession: TerminalLine[] = [
  { cmd: 'lokal project create shop', comment: 'https://shop.lokal; becomes the current project' },
  { cmd: 'lokal service add @ 3000', comment: 'the apex itself: https://shop.lokal → localhost:3000' },
  { cmd: 'lokal service add api 4000', comment: 'https://api.shop.lokal' },
  { cmd: 'lokal service add v1.api 4001', comment: 'nested names work too' },
  { cmd: 'lokal service add db-ui 8080 --host 127.0.0.1' },
  { cmd: 'lokal run web -- npm run dev', comment: 'picks a port, routes https://web.shop.lokal, starts the server' },
  { cmd: 'lokal list', comment: 'every project, service and URL' },
  { cmd: 'lokal open api', comment: 'open it in your browser' },
  { cmd: 'lokal service update api --port 4002' },
  { cmd: 'lokal service remove api' },
];

// README.md, "Node, Python, curl and friends", "Vite …", FAQ (Docker) and "Why lokal".
export const stack: { title: string; text: string; code: string; lang: string; href: string }[] = [
  {
    title: 'Vite',
    text: 'Vite rejects Host headers it doesn’t recognise. Allow your lokal names, or have lokal send `Host: localhost:<port>` upstream.',
    code: 'lokal service add web 5173 --host-header upstream',
    lang: 'sh',
    href: '/docs/guides/frameworks',
  },
  {
    title: 'Next.js & webpack',
    text: 'Hot reload runs over WebSockets, and WebSockets pass straight through the proxy.',
    code: 'lokal service add web 3000',
    lang: 'sh',
    href: '/docs/guides/frameworks',
  },
  {
    title: 'Node & Python',
    text: 'They ship their own certificate bundles. `lokal env` sets `NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE`, and public sites keep working.',
    code: 'eval "$(lokal env)"',
    lang: 'sh',
    href: '/docs/guides/lokal-env',
  },
  {
    title: 'OAuth & cookies',
    text: 'Each service has its own hostname, so redirect URLs, CORS and cookies behave like production instead of sharing `localhost` across ports.',
    code: 'lokal service add auth 9000',
    lang: 'sh',
    href: '/docs/guides/projects-and-services',
  },
  {
    title: 'curl',
    text: 'macOS’s own curl trusts lokal as soon as setup finishes: no `-k`, no `--cacert`. For other curl builds, `lokal env` sets `CURL_CA_BUNDLE`.',
    code: 'curl https://api.myapp.lokal',
    lang: 'sh',
    href: '/docs/guides/lokal-env',
  },
  {
    title: 'Docker',
    text: 'Works for ports you publish on your Mac. Containers can’t resolve lokal’s names themselves.',
    code: 'lokal service add app 8080',
    lang: 'sh',
    href: '/docs/faq',
  },
  {
    title: 'IPv6-only servers',
    text: '`localhost` upstreams work whether your server listens on `127.0.0.1` or only on `::1`. Pin one with `--host`.',
    code: 'lokal service add db-ui 8080 --host 127.0.0.1',
    lang: 'sh',
    href: '/docs/guides/projects-and-services',
  },
  {
    title: 'Streaming',
    text: 'Server-sent events, long polling, streaming uploads and HTTP/2 all pass through.',
    code: 'curl -N https://events.myapp.lokal',
    lang: 'sh',
    href: '/docs/concepts/how-it-works',
  },
];

// docs/live-test-reports/macos-27.0-lokal-tld-g2-full.json, check L23 "Performance".
export const perfReport = {
  macos: '27.0',
  lokal: '0.1.0',
  checks: '24 of 25 checks passed; the optional Safari check was skipped.',
};

export const perfStats = [
  { value: 13.68, decimals: 2, unit: 'ms', label: 'service add, p95', target: 'target < 300ms' },
  { value: 0.19, decimals: 2, unit: 'ms', label: 'proxy overhead, p95', target: 'target < 2ms' },
  { value: 17.62, decimals: 2, unit: 'MB', label: 'memory with 30 services', target: 'target < 30MB' },
  { value: 104.45, decimals: 2, unit: 'ms', label: 'restart → serving every name', target: 'target < 500ms' },
];

export const perfLines: TerminalLine[] = [
  'service add p95 13.68ms (< 300ms) ok',
  'first request after add: ok',
  'lookup first 1.25ms (< 20ms) ok',
  'lookup repeat 0.53ms (< 5ms) ok',
  'proxy overhead p95 0.19ms (< 2ms) ok',
  'resumed TLS p95 1.67ms (< 10ms) ok',
  'lokal list p95 6.54ms (< 30ms) ok',
  'lokal status p95 6.44ms (< 30ms) ok',
  'RSS with 30 services 17.62MB (< 30MB) ok',
  'idle CPU over 10s 0.00% (< 0.5%) ok',
  'restart → serving all names 104.45ms (< 500ms) ok',
  'all names served after restart: ok',
].map((out) => ({ out }));

// README.md, "Using lokal with coding agents" (verbatim snippet).
export const agentsSnippet = `## Local HTTPS URLs (lokal)
- This repo's dev servers run behind lokal: https://web.myapp.lokal → localhost:3000,
  https://api.myapp.lokal → localhost:4000.
- Set up routes with \`lokal project create myapp\` and \`lokal service add <sub> <port>\` (safe to re-run).
- Or start a dev server on a port lokal picks: \`lokal run <sub> -- <dev command>\`, then wait for
  \`✓ ready:\` on stderr.
- Or, with a \`lokal.toml\` in the repo: run \`lokal up --yes\` in the background and wait for \`✓ ready:\` on stdout.
  Stop it with Ctrl-C or SIGTERM; \`lokal down\` removes its routes.
- One \`lokal up\` runs per project. If it says \`lokal up is already running\`, use that stack and don't stop it;
  \`lokal logs <sub>\` shows its output.
- When a .lokal URL fails, \`lokal logs <sub> -n 50\` shows that service's recent output (for services that
  \`lokal up\` started).
- When a URL fails, run \`lokal doctor\`. Never run \`lokal setup\` or sudo; ask the user.`;

// docs/SPEC.md §4.9.
export const exitCodes = [
  { code: 0, meaning: 'Success' },
  { code: 1, meaning: 'Runtime error, or doctor found a failure' },
  { code: 2, meaning: 'Usage error' },
  { code: 3, meaning: "Setup hasn't run" },
  { code: 4, meaning: 'Needs sudo, but sudo isn’t possible' },
];
