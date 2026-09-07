import type { SVGProps } from "react";
import { clsx } from "clsx";

/**
 * The Portal's icons, inline: lucide-react is not a dependency and one more package for a
 * dozen glyphs is not worth its supply chain. Every path is a 24-unit stroke icon so the set
 * reads as one. Decorative by default (`aria-hidden`); pass a `title` prop for a labelled one.
 */
const PATHS: Record<string, string> = {
  flows: "M4 6h6v6H4zM14 12h6v6h-6zM10 9h4M12 9v3M12 15h2",
  spaces: "M3 7l9-4 9 4-9 4-9-4zM3 12l9 4 9-4M3 17l9 4 9-4",
  endpoints: "M4 12h4M16 12h4M8 12a4 4 0 0 1 8 0 4 4 0 0 1-8 0zM12 4v4M12 16v4",
  datasources: "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  pipelines: "M3 6h5l3 6-3 6H3M13 6h5l3 6-3 6h-5",
  dashboards: "M3 4h18v6H3zM3 14h8v6H3zM15 14h6v6h-6z",
  apps: "M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z",
  approvals: "M9 12l2 2 4-4M12 3l7 3v6c0 4-3 7-7 9-4-2-7-5-7-9V6z",
  access: "M8 11V8a4 4 0 1 1 8 0v3M5 11h14v10H5zM12 15v2",
  models: "M12 3l8 4.5v9L12 21l-8-4.5v-9zM12 12l8-4.5M12 12v9M12 12L4 7.5",
  ckan: "M4 5h16v14H4zM8 9h8M8 13h5",
  federation: "M6 5a2 2 0 1 0 0 4 2 2 0 0 0 0-4zM18 5a2 2 0 1 0 0 4 2 2 0 0 0 0-4zM12 15a2 2 0 1 0 0 4 2 2 0 0 0 0-4zM7.5 8.5l3.5 5.5M16.5 8.5 13 14",
  playground: "M4 17l4-8 4 5 3-3 5 6H4z",
  menu: "M4 6h16M4 12h16M4 18h16",
  close: "M6 6l12 12M18 6 6 18",
  chevronDown: "m6 9 6 6 6-6",
  chevronRight: "m9 6 6 6-6 6",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21a8 8 0 0 1 16 0",
  plus: "M12 5v14M5 12h14",
  external: "M14 4h6v6M20 4l-9 9M18 13v6H5V6h6",
  copy: "M9 9h10v11H9zM5 15V4h10",
  check: "m5 12 5 5 9-10",
  warning: "M12 4 2 20h20zM12 10v4M12 17v.5",
  info: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 11v5M12 8v.5",
  error: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM9 9l6 6M15 9l-6 6",
  search: "M10 4a6 6 0 1 0 0 12 6 6 0 0 0 0-12zM20 20l-5.5-5.5",
  refresh: "M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5",
  inbox: "M3 13h5l2 3h4l2-3h5M3 13V6h18v7M3 13v5h18v-5",
  globe: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5",
  arrowUp: "M12 19V5M5 12l7-7 7 7",
  arrowDown: "M12 5v14M5 12l7 7 7-7",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6",
  logout: "M10 4H5v16h5M14 8l4 4-4 4M18 12H9",
};

export type IconName = keyof typeof PATHS;

export interface IconProps extends Omit<SVGProps<SVGSVGElement>, "name"> {
  name: IconName;
  /** An accessible name; without one the icon is decorative. */
  title?: string;
}

export function Icon({ name, title, className, ...rest }: IconProps): React.JSX.Element {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : "true"}
      role={title ? "img" : undefined}
      className={clsx("size-5 shrink-0", className)}
      {...rest}
    >
      {title ? <title>{title}</title> : null}
      <path d={PATHS[name]} />
    </svg>
  );
}
