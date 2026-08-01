import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Button } from "./Button";
import { Menu } from "./Menu";

/*
 * What is worth testing about a menu built on a headless library is the
 * contract Kodabi depends on, not base-ui's own coverage of itself: that the
 * trigger is still ONE button (the `render` composition, which is easy to get
 * wrong and produces a nested-button DOM when you do), that the keyboard can
 * reach and fire a row, and that Escape gives focus back to where it came from.
 */
function Harness({ onFile = () => {} }: { onFile?: () => void }) {
  return (
    <Menu.Root>
      <Menu.Trigger render={<Button variant="quiet">File</Button>} />
      <Menu.Content>
        <Menu.Item onClick={onFile}>Briarwood Golf</Menu.Item>
        <Menu.Item>Riverbend Deck</Menu.Item>
        <Menu.Separator />
        <Menu.Item>New project…</Menu.Item>
      </Menu.Content>
    </Menu.Root>
  );
}

describe("Menu", () => {
  it("composes the trigger into one button rather than nesting two", async () => {
    render(<Harness />);

    const trigger = screen.getByRole("button", { name: "File" });
    // The Grove button's own chrome, on the element base-ui wired up.
    expect(trigger).toHaveClass("rounded-button");
    expect(trigger.querySelector("button")).toBeNull();
    expect(trigger).toHaveAttribute("aria-haspopup");
  });

  it("opens on the trigger and lists its items", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "File" }));

    // `findBy`, not `getBy`: the popup mounts through the positioner, which
    // measures before it paints, so the menu lands a frame after the click.
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    expect(screen.getAllByRole("menuitem")).toHaveLength(3);
  });

  it("fires an item from the keyboard", async () => {
    const user = userEvent.setup();
    const onFile = vi.fn();
    render(<Harness onFile={onFile} />);

    await user.click(screen.getByRole("button", { name: "File" }));
    await user.keyboard("{ArrowDown}{Enter}");

    expect(onFile).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("closes on Escape and returns focus to the trigger", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const trigger = screen.getByRole("button", { name: "File" });
    await user.click(trigger);
    expect(await screen.findByRole("menu")).toBeInTheDocument();

    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
    expect(trigger).toHaveFocus();
  });
});
