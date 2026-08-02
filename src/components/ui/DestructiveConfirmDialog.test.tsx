import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { DestructiveConfirmDialog } from "./DestructiveConfirmDialog";

/** A trigger that mounts the dialog on click, so focus hand-off and restore can
 * be exercised the way a real caller drives it (open on a button, close on
 * unmount). */
function Harness({
  onConfirm = () => {},
  busy = false,
  error = null,
  subject = "Sprinkler quotes and the 9th green rebuild",
}: {
  onConfirm?: () => void;
  busy?: boolean;
  error?: string | null;
  subject?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open
      </button>
      {open && (
        <DestructiveConfirmDialog
          title="Delete this thing?"
          subject={subject}
          confirmLabel="Delete thing"
          busyLabel="Deleting…"
          busy={busy}
          error={error}
          onConfirm={onConfirm}
          onClose={() => setOpen(false)}
        >
          <p>The thing is deleted from your vault.</p>
        </DestructiveConfirmDialog>
      )}
    </>
  );
}

describe("DestructiveConfirmDialog", () => {
  it("names the dialog by its title and focuses Cancel on open", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    const dialog = screen.getByRole("dialog", { name: "Delete this thing?" });
    // Cancel is the default action of a confirmation, so the keyboard's first
    // Enter dismisses rather than destroys.
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
  });

  it("names the acted-on thing in its own strip and owns the permanence warning", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    const dialog = screen.getByRole("dialog");
    const subject = within(dialog).getByText("Sprinkler quotes and the 9th green rebuild");
    // The strip truncates, so the full name has to stay reachable.
    expect(subject).toHaveAttribute("title", "Sprinkler quotes and the 9th green rebuild");
    // The warning is the dialog's own line, not something each caller remembers
    // to write.
    expect(within(dialog).getByText("This cannot be undone.")).toBeInTheDocument();
  });

  it("keeps the warning when there is no subject to name", () => {
    render(
      <DestructiveConfirmDialog
        title="Delete this thing?"
        confirmLabel="Delete thing"
        busyLabel="Deleting…"
        busy={false}
        error={null}
        onConfirm={() => {}}
        onClose={() => {}}
      >
        <p>The thing is deleted from your vault.</p>
      </DestructiveConfirmDialog>,
    );

    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("This cannot be undone.")).toBeInTheDocument();
    expect(
      within(dialog).queryByText("Sprinkler quotes and the 9th green rebuild"),
    ).not.toBeInTheDocument();
  });

  it("puts Cancel before the destructive action, so nothing is passed over to reach it", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    const dialog = screen.getByRole("dialog");
    const cancel = within(dialog).getByRole("button", { name: "Cancel" });
    const confirm = within(dialog).getByRole("button", { name: "Delete thing" });
    expect(cancel.compareDocumentPosition(confirm)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("closes on Escape and restores focus to the opener", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const opener = screen.getByRole("button", { name: "Open" });
    await user.click(opener);
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
  });

  it("dismisses on a scrim press", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.click(screen.getByTestId("dialog-scrim"));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("runs the action on confirm", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Harness onConfirm={onConfirm} />);

    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.click(screen.getByRole("button", { name: "Delete thing" }));

    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("shows an error inside the dialog via an alert", async () => {
    const user = userEvent.setup();
    render(<Harness error="Couldn't delete: it is locked" />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Couldn't delete: it is locked");
  });

  it("goes inert while busy without dropping the confirm from the tab order", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Harness busy onConfirm={onConfirm} />);

    await user.click(screen.getByRole("button", { name: "Open" }));

    // While busy the confirm control reads its busy label and is inert to
    // assistive tech, but stays focusable (aria-disabled, not native disabled).
    const confirm = screen.getByRole("button", { name: "Deleting…" });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    expect(confirm).toHaveAttribute("aria-busy", "true");
    expect(confirm).not.toBeDisabled();

    await user.click(confirm);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
