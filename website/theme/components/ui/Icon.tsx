import type { SVGProps } from 'react';
import { iconPaths, type IconName } from './icons';

export type { IconName };

type Props = Omit<SVGProps<SVGSVGElement>, 'name'> & {
  name: IconName;
  size?: number;
};

// Extra props are forwarded so a parent's styles can target the icon via `className`.
export function Icon({ name, size = 24, className, ...rest }: Props) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
      {...rest}
      dangerouslySetInnerHTML={{ __html: iconPaths[name] }}
    />
  );
}
