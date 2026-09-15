/**
 * The UI's half of the roles matrix (T-0751, PF-50, PF-58, CC-34): for the same roles and kinds as
 * tests/roles_matrix_tests.rs, the Portal offers a control exactly when the API takes the action.
 */
import { describe, expect, it } from "vitest";
import { approvalStanding } from "../src/api/approval";
import { allows } from "../src/api/permissions";
import type { Effective, Verb } from "../src/api/permissions";

const KINDS = ["Endpoint", "Pipeline", "DataSource", "DataModel", "RoleBinding", "Dashboard", "App"];

const ROLES: Record<string, Verb[]> = {
  viewer: [],
  editor: ["propose"],
  steward: ["propose", "approve"],
  admin: ["propose", "approve", "delete"],
};

function effective(role: string): Effective {
  const verbs = ROLES[role];
  return {
    bootstrap: false,
    project: "helsinki",
    grants: verbs.length === 0 ? [] : [{ binding: `${role}-binding`, role, rule: { kinds: KINDS, verbs } as never }],
  };
}

function change(kind: string, authorEmail: string) {
  return { summary: { params: { kind, name: "proposed" } }, author: { email: authorEmail } };
}

describe("the controls each role is offered", () => {
  for (const role of Object.keys(ROLES)) {
    for (const kind of KINDS) {
      it(`${role} on ${kind}: propose, delete and approve as the API allows`, () => {
        const data = effective(role);
        const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
        for (const verb of ["propose", "delete", "approve"] as const) {
          expect(allows(data, kind, verb), verb).toBe(ROLES[role].includes(verb));
        }

        const other = approvalStanding(permissions, `${role}@hel.fi`, change(kind, "someone@hel.fi"));
        expect(other.block).toBe(ROLES[role].includes("approve") ? null : "needsRole");

        const own = approvalStanding(permissions, `${role}@hel.fi`, change(kind, `${role}@hel.fi`));
        const expected = !ROLES[role].includes("approve") ? "needsRole" : ROLES[role].includes("delete") ? null : "ownProposal";
        expect(own.block).toBe(expected);
        expect(own.ownAsAdministrator).toBe(expected === null);
      });
    }
  }

  it("the bootstrap group is offered everything but approving its own change", () => {
    const data: Effective = { bootstrap: true, project: "helsinki", grants: [] };
    const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
    expect(KINDS.every((kind) => allows(data, kind, "delete"))).toBe(true);
    expect(approvalStanding(permissions, "admin@hel.fi", change("Pipeline", "admin@hel.fi")).block).toBe("ownProposal");
  });
});
