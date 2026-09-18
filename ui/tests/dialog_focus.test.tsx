/** The shared dialog hands focus back to what opened it (T-1490, UI-44). */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { Dialog } from "../src/components/ui";

function Page({ removeOpener = false }: { removeOpener?: boolean }) {
  const [open, setOpen] = useState(false);
  const [gone, setGone] = useState(false);
  return (
    <main>
      {gone ? null : (
        <button
          onClick={() => {
            setOpen(true);
            if (removeOpener) setTimeout(() => setGone(true), 0);
          }}
        >
          New thing
        </button>
      )}
      <Dialog open={open} onOpenChange={setOpen} title="New thing" closeLabel="Close">
        <input aria-label="Name" />
      </Dialog>
    </main>
  );
}

describe("the shared dialog", () => {
  it("returns focus to the button that opened it, on Escape and on Close", async () => {
    const user = userEvent.setup();
    render(<Page />);
    const opener = screen.getByRole("button", { name: "New thing" });
    for (const close of [() => user.keyboard("{Escape}"), () => user.click(screen.getByRole("button", { name: "Close" }))]) {
      await user.click(opener);
      await screen.findByRole("dialog");
      await close();
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
      expect(opener).toHaveFocus();
    }
  });

  it("does not throw or steal focus when the opener left the page meanwhile", async () => {
    const user = userEvent.setup();
    render(<Page removeOpener />);
    await user.click(screen.getByRole("button", { name: "New thing" }));
    await screen.findByRole("dialog");
    await waitFor(() => expect(screen.queryByRole("button", { name: "New thing" })).toBeNull());
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
