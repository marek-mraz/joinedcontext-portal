import { allows, type Effective, type Verb } from "./permissions";

/** Why the approve button is off, or `null` when the API would take the approval. */
export type ApprovalBlock = "needsRole" | "ownProposal" | null;

export interface ApprovalStanding {
  block: ApprovalBlock;
  /** The caller wrote the change and approves it as an administrator of its kind (PF-58). */
  ownAsAdministrator: boolean;
}

/**
 * What the approval rules say about the caller and one change (CC-34, PF-50, PF-58): an approver of
 * the kind may approve someone else's change, and their own only when they may also delete that
 * kind. A convenience for the button; the API decides on its own.
 */
export function approvalStanding(
  permissions: { data?: Effective; can: (kind: string, verb: Verb) => boolean },
  callerEmail: string | undefined,
  change: { summary: { params: unknown }; author: { email?: string | null } },
): ApprovalStanding {
  const kind = changedKind(change);
  if (!permissions.can(kind, "approve")) {
    return { block: "needsRole", ownAsAdministrator: false };
  }
  const own = Boolean(
    callerEmail &&
      change.author.email &&
      callerEmail.toLowerCase() === change.author.email.toLowerCase(),
  );
  if (!own) {
    return { block: null, ownAsAdministrator: false };
  }
  // The bootstrap group is not a binding, so it does not administer a kind (PF-58).
  const administers =
    permissions.data?.bootstrap !== true &&
    allows(permissions.data, kind, "approve") &&
    allows(permissions.data, kind, "delete");
  return administers
    ? { block: null, ownAsAdministrator: true }
    : { block: "ownProposal", ownAsAdministrator: false };
}

/** The kind of the manifest a change touches, as its summary names it; `*` when it names none. */
export function changedKind(change: { summary: { params: unknown } }): string {
  const kind = (change.summary.params as { kind?: unknown } | null)?.kind;
  return typeof kind === "string" && kind !== "" ? kind : "*";
}
