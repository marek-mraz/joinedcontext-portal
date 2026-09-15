import { Fragment } from "react";
import type { JSX, ReactNode } from "react";
import { useRouterState } from "@tanstack/react-router";

/** What the assistant leaves in the address for the page it opens (AG-73, AG-77). */
const HAND_OFF = ["edit", "delete", "grant", "draft", "space"] as const;

/**
 * Its page, mounted afresh for each hand-off in the address. A page takes what the assistant
 * handed it once, when it mounts, so a person already on that page would otherwise see nothing
 * open when the assistant sends them there again.
 */
export function HandOff({ children }: { children: ReactNode }): JSX.Element {
  const key = useRouterState({
    select: (state) => {
      const search = new URLSearchParams(state.location.searchStr);
      return HAND_OFF.map((name) => search.get(name) ?? "").join("\n");
    },
  });
  return <Fragment key={key}>{children}</Fragment>;
}
