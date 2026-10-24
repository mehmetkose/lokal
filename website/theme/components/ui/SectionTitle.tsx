// Monospace "// eyebrow" + display heading, used at the top of most sections.
import { cx } from '../../lib/cx';

type Props = {
  subtitle?: string;
  /** HTML: may contain spans such as <span class="c-orange">…</span>. */
  title: string;
  align?: 'left' | 'center';
  /** Max width of the block, e.g. 650 (px). */
  width?: number;
  tag?: 'h1' | 'h2' | 'h3';
  size?: 'h1' | 'h2' | 'h3';
  subtitleColor?: 'orange' | 'yellow' | 'green' | 'white';
  titleColor?: 'green' | 'white' | 'dark';
  className?: string;
};

export function SectionTitle({
  subtitle,
  title,
  align = 'center',
  width,
  tag: Tag = 'h2',
  size = 'h2',
  subtitleColor = 'orange',
  titleColor = 'green',
  className,
}: Props) {
  return (
    <div
      className={cx('title-block', align === 'center' && 'title-block--center', className)}
      style={width ? { maxWidth: `${width}px` } : undefined}
      data-reveal=""
    >
      {subtitle && (
        <p className={cx('subtitle', subtitleColor !== 'orange' && `c-${subtitleColor}`)}>
          <span aria-hidden="true">// </span>
          {subtitle}
        </p>
      )}
      <Tag className={`${size} c-${titleColor}`} dangerouslySetInnerHTML={{ __html: title }} />
    </div>
  );
}
