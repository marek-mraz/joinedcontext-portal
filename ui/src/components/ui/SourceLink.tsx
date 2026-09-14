import type { JSX } from "react";
import { buttonClass } from "./Button";
import { Icon } from "./icons";

export interface SourceLinkProps {
  href: string;
  /** The link's accessible name, also its tooltip: the icon alone is what shows. */
  label: string;
}

/** The manifest or the code in the forge, as one icon beside the row or the run (MF-11). */
export function SourceLink({ href, label }: SourceLinkProps): JSX.Element {
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      aria-label={label}
      title={label}
      className={buttonClass("ghost", "sm", "text-primary")}
    >
      <Icon name="git" className="size-4" />
    </a>
  );
}
