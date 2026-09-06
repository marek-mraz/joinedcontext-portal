import { useState } from "react";
import type { JSX } from "react";
import { ExportModal } from "./ExportModal";
import type { ExportTarget } from "./ExportModal";

/** The button that opens the download modal, wherever a page offers an export (MF-16). */
export function ExportButton({
  project,
  target,
  label,
  className,
}: {
  project: string;
  target: ExportTarget;
  label: string;
  className?: string;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className={
          className ??
          "rounded border border-border px-3 py-1.5 text-sm font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        }
      >
        {label}
      </button>
      <ExportModal project={project} target={target} open={open} onOpenChange={setOpen} />
    </>
  );
}
