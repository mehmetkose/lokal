// Rspress always bundles its default theme's base CSS (a Tailwind-style reset,
// body font and colours, Shiki line padding) through `source.preEntry`. This
// site has its own design and reset in theme/global.css, so drop it.
const DEFAULT_STYLES = /[\\/]@rspress[\\/]core[\\/]dist[\\/]theme[\\/]styles[\\/]index\.js$/;

type SourceConfig = { preEntry?: string | string[] };
type RsbuildConfig = { source?: SourceConfig; environments?: Record<string, { source?: SourceConfig }> };

const strip = (source?: SourceConfig) => {
  if (!source?.preEntry) return false;
  const entries = Array.isArray(source.preEntry) ? source.preEntry : [source.preEntry];
  const kept = entries.filter((entry) => !DEFAULT_STYLES.test(entry));
  source.preEntry = kept;
  return kept.length !== entries.length;
};

export function dropDefaultStyles() {
  return {
    name: 'lokal:drop-default-styles',
    setup(api: { modifyRsbuildConfig: (fn: (config: RsbuildConfig) => void) => void }) {
      api.modifyRsbuildConfig((config) => {
        let dropped = strip(config.source);
        for (const env of Object.values(config.environments ?? {})) dropped = strip(env.source) || dropped;
        if (!dropped) throw new Error('drop-default-styles: Rspress default styles not found in source.preEntry');
      });
    },
  };
}
