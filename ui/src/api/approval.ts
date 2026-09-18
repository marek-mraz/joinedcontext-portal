import { allows, type Effective, type Rule, type Verb } from "./permissions";

/** Why the approve button is off, or `null` when the API would take the approval. */
export type ApprovalBlock = "needsRole" | "ownProposal" | "needsPublisher" | null;

/** One field the change alters, as the plan lists it. */
interface PlannedField {
  path: string;
  from?: unknown;
  to?: unknown;
}

/** The audience an Endpoint carries when its data is open to anyone (EP-14). */
const PUBLIC = "public";

/**
 * Whether this change lets an endpoint out to the public: it gives `spec.audience` the value
 * `public`, on creation or by an update from another audience (EP-76).
 */
export function makesPublic(change: { planFields?: PlannedField[] | null }): boolean {
  return (change.planFields ?? []).some(
    (field) => field.path.endsWith("audience") && field.to === PUBLIC,
  );
}

/**
 * Whether one of the caller's grants approves an Endpoint whose audience is `public` (PF-71):
 * a rule with no constraint on the audience, or one whose constraint the value satisfies. The
 * page only says which role is missing; the API is what refuses (PF-51).
 */
function mayPublish(effective: Effective | undefined): boolean {
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return true;
  }
  return effective.grants.some((grant) => {
    const rule = grant.rule as Rule & {
      constraints?: { field?: string; in?: string[]; notIn?: string[]; equals?: string }[];
    };
    if (!rule.kinds?.includes("Endpoint") || !rule.verbs?.includes("approve")) {
      return false;
    }
    return (rule.constraints ?? []).every((constraint) => {
      if (constraint.field !== "spec.audience") {
        return true;
      }
      if (constraint.equals !== undefined) {
        return constraint.equals === PUBLIC;
      }
      if (constraint.in !== undefined) {
        return constraint.in.includes(PUBLIC);
      }
      if (constraint.notIn !== undefined) {
        return !constraint.notIn.includes(PUBLIC);
      }
      return true;
    });
  });
}

export interface ApprovalStanding {
  block: ApprovalBlock;
  /** The caller wrote the change and approves it as an administrator of its kind (PF-58). */
  ownAsAdministrator: boolean;
}

/** Whether the caller wrote the change: its author is the caller's own e-mail (CC-34). */
export function isOwn(callerEmail: string | null | undefined, change: { author: { email?: string | null } }): boolean {
  return Boolean(
    callerEmail && change.author.email && callerEmail.toLowerCase() === change.author.email.toLowerCase(),
  );
}

/**
 * What the approval rules say about the caller and one change (CC-34, PF-50, PF-58): an approver of
 * the kind may approve someone else's change, and their own only when they may also delete that
 * kind. A convenience for the button; the API decides on its own.
 */
export function approvalStanding(
  permissions: { data?: Effective; can: (kind: string, verb: Verb) => boolean },
  callerEmail: string | undefined,
  change: {
    summary: { params: unknown };
    author: { email?: string | null };
    planFields?: PlannedField[] | null;
  },
): ApprovalStanding {
  const kind = changedKind(change);
  if (!permissions.can(kind, "approve")) {
    return { block: "needsRole", ownAsAdministrator: false };
  }
  // Letting data out to the public is a right of its own, and the page says which role holds
  // it rather than only that the button is off (EP-76, PF-71, UI-44).
  if (kind === "Endpoint" && makesPublic(change) && !mayPublish(permissions.data)) {
    return { block: "needsPublisher", ownAsAdministrator: false };
  }
  if (!isOwn(callerEmail, change)) {
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
