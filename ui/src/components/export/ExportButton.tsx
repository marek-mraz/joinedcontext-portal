import { useState } from "react";
import type { JSX } from "react";
import { ExportModal } from "./ExportModal";
import type { ExportTarget } from "./ExportModal";
import { Button } from "../ui";
import type { ButtonSize, ButtonVariant } from "../ui";

/** The button that opens the download modal, wherever a page offers an export (MF-16). */
export function ExportButton({
  project,
  target,
  label,
  className,
  variant = "secondary",
  size = "md",
}: {
  project: string;
  target: ExportTarget;
  label: string;
  className?: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant={variant} size={size} className={className} onClick={() => setOpen(true)}>
        {label}
      </Button>
      <ExportModal project={project} target={target} open={open} onOpenChange={setOpen} />
    </>
  );
}
