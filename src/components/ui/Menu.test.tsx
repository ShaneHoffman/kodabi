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

  /*
   * The composition's sharpest edge. A row that files a note marks its trigger
   * `loading` rather than `disabled`, so the control keeps the focus base-ui
   * just handed back to it (docs/UI_CONVENTIONS.md §4) — but base-ui opens this
   * menu on MOUSEDOWN and on ArrowDown, and a busy `Button` used to cancel only
   * the click. The trigger looked inert and reopened on the first press,
   * offering a second run of an action already in flight.
   */
  it("does not open while the trigger is busy", async () => {
    const user = userEvent.setup();
    render(
      <Menu.Root>
        <Menu.Trigger render={<Button loading loadingLabel="Filing…">File</Button>} />
        <Menu.Content>
          <Menu.Item>Briarwood Golf</Menu.Item>
        </Menu.Content>
      </Menu.Root>,
    );

    const trigger = screen.getByRole("button", { name: "Filing…" });
    await user.click(trigger);
    trigger.focus();
    await user.keyboard("{ArrowDown}");

    // `aria-expanded`, not the absence of the popup: the popup mounts a frame
    // after its trigger fires, so asserting it is missing would pass on the
    // broken build too. base-ui writes the open decision to the trigger
    // synchronously, which makes this the assertion that can actually fail.
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    // Still the user's place in the page, which is why it is busy and not
    // disabled in the first place.
    expect(trigger).not.toBeDisabled();
    expect(trigger).toHaveFocus();
  });

  it("carries its row appearances as variants, not as call-site overrides", async () => {
    // Font size and text colour belong to the row recipe, so a call site
    // passing `text-[11.5px]` alongside the recipe's own `text-[13px]` is
    // resolved by Tailwind's emission order rather than by the className —
    // and both of these lost that flip when they were written that way
    // (docs/UI_CONVENTIONS.md §4). As variants there is exactly one size
    // utility and one colour utility on the element, so nothing can conflict.
    const user = userEvent.setup();
    render(
      <Menu.Root>
        <Menu.Trigger render={<Button>File</Button>} />
        <Menu.Content>
          <Menu.Item variant="suggested">Briarwood Golf</Menu.Item>
          <Menu.Item>Riverbend Deck</Menu.Item>
          <Menu.Item variant="foot">New project…</Menu.Item>
        </Menu.Content>
      </Menu.Root>,
    );

    await user.click(screen.getByRole("button", { name: "File" }));
    const [suggested, plain, foot] = await screen.findAllByRole("menuitem");

    const sizes = (element: HTMLElement) =>
      Array.from(element.classList).filter((name) => name.startsWith("text-["));
    const colours = (element: HTMLElement) =>
      Array.from(element.classList).filter((name) => name.startsWith("text-ink"));

    for (const row of [suggested, plain, foot]) {
      expect(sizes(row)).toHaveLength(1);
      expect(colours(row)).toHaveLength(1);
    }
    // The suggestion reads brighter than its siblings even when the highlight
    // is elsewhere: it is a claim about content, not a hover state.
    expect(suggested).toHaveClass("bg-wash", "text-ink");
    expect(plain).toHaveClass("text-[13px]", "text-ink-dim");
    expect(foot).toHaveClass("text-[11.5px]", "text-ink-faint");
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
