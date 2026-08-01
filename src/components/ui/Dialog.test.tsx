import { useRef, useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Button } from "./Button";
import { Dialog } from "./Dialog";

/*
 * The four promises a caller makes to its users when it opens a Dialog, and
 * which base-ui rather than Kodabi now keeps: focus goes in, Escape and the
 * scrim bring it back out, and focus lands where the dialog says rather than on
 * whatever happens to be first.
 *
 * `DestructiveConfirmDialog.test.tsx` covers the same ground through a real
 * caller; this covers the primitive on its own, including the `initialFocus`
 * override that the destructive dialog depends on.
 */
function Harness({ focusSecond = false }: { focusSecond?: boolean }) {
  const [open, setOpen] = useState(false);
  const secondRef = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open
      </button>
      {open && (
        <Dialog
          open
          onDismiss={() => setOpen(false)}
          label="Move this note?"
          initialFocus={focusSecond ? secondRef : undefined}
        >
          <Button>First</Button>
          <Button ref={secondRef}>Second</Button>
        </Dialog>
      )}
    </>
  );
}

describe("Dialog", () => {
  it("is a named modal dialog when open", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open" }));

    const dialog = screen.getByRole("dialog", { name: "Move this note?" });
    expect(within(dialog).getByRole("button", { name: "First" })).toBeInTheDocument();
  });

  it("takes focus on open, to the first control by default", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    // The hand-off runs after the popup is in the document, so it is awaited
    // rather than read on the same tick as the click.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "First" })).toHaveFocus();
    });
  });

  it("honours initialFocus, so a destructive dialog can open on its safe action", async () => {
    const user = userEvent.setup();
    render(<Harness focusSecond />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Second" })).toHaveFocus();
    });
  });

  it("dismisses on Escape and on a scrim press, restoring focus to the opener", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const opener = screen.getByRole("button", { name: "Open" });

    await user.click(opener);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(opener).toHaveFocus();

    await user.click(opener);
    await user.click(screen.getByTestId("dialog-scrim"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
