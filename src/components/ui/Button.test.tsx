import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FormEvent } from "react";
import { describe, expect, it, vi } from "vitest";
import { Button } from "./Button";

/*
 * `loading` exists for one reason: a pending action must not cost the user
 * their place in the page (docs/DESIGN_SYSTEM.md §6). Keeping the control
 * mounted is only half of it — the native `disabled` attribute blurs a focused
 * element just as unmounting it does — so these assert the control stays
 * focusable AND stays inert, which is the pair that makes the rule true.
 */
describe("Button", () => {
  it("keeps a busy control focusable instead of disabling it", () => {
    render(
      <Button loading loadingLabel="Saving…">
        Save
      </Button>,
    );

    const button = screen.getByRole("button");
    expect(button).not.toBeDisabled();
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAttribute("aria-busy", "true");
    expect(button).toHaveTextContent("Saving…");

    button.focus();
    expect(button).toHaveFocus();
  });

  it("swallows activation while busy", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(
      <Button loading loadingLabel="Saving…" onClick={onClick}>
        Save
      </Button>,
    );

    await user.click(screen.getByRole("button"));
    // Enter as well as the pointer: a focusable button is one a keyboard can
    // still reach, which is the whole point of not disabling it.
    screen.getByRole("button").focus();
    await user.keyboard("{Enter}");

    expect(onClick).not.toHaveBeenCalled();
  });

  /*
   * Click is not the only way a composed control is activated. `Menu.Trigger`
   * merges its own handlers into this button and opens on MOUSEDOWN and on
   * ArrowDown, neither of which passes through the click guard above — so a
   * busy trigger stayed fully operable while looking inert. The handlers are
   * dropped rather than cancelled, because a cancelled mousedown would refuse
   * the focus `loading` exists to keep and a cancelled keydown would eat Tab.
   */
  it("swallows the pointer and key activation a composed trigger opens on", async () => {
    const user = userEvent.setup();
    const onMouseDown = vi.fn();
    const onPointerDown = vi.fn();
    const onKeyDown = vi.fn();
    render(
      <Button
        loading
        loadingLabel="Saving…"
        onMouseDown={onMouseDown}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
      >
        Save
      </Button>,
    );

    const button = screen.getByRole("button");
    await user.click(button);
    button.focus();
    await user.keyboard("{ArrowDown}");

    expect(onMouseDown).not.toHaveBeenCalled();
    expect(onPointerDown).not.toHaveBeenCalled();
    expect(onKeyDown).not.toHaveBeenCalled();
    // Dropped, not cancelled: the press still handed the button its focus.
    expect(button).toHaveFocus();
  });

  it("passes the pointer and key handlers through when it is not busy", async () => {
    const user = userEvent.setup();
    const onMouseDown = vi.fn();
    const onKeyDown = vi.fn();
    render(
      <Button onMouseDown={onMouseDown} onKeyDown={onKeyDown}>
        Save
      </Button>,
    );

    await user.click(screen.getByRole("button"));
    await user.keyboard("{ArrowDown}");

    expect(onMouseDown).toHaveBeenCalledTimes(1);
    expect(onKeyDown).toHaveBeenCalledTimes(1);
  });

  it("does not submit its form while busy", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn((event: FormEvent) => event.preventDefault());
    render(
      <form onSubmit={onSubmit}>
        <Button type="submit" loading loadingLabel="Saving…">
          Save
        </Button>
      </form>,
    );

    await user.click(screen.getByRole("button"));

    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("falls back to its own children when no loading label is given", () => {
    render(<Button loading>Retry</Button>);

    expect(screen.getByRole("button")).toHaveTextContent("Retry");
  });

  it("still disables genuinely, and `disabled` beats `loading`", () => {
    const { rerender } = render(<Button disabled>Save</Button>);

    expect(screen.getByRole("button")).toBeDisabled();

    // A caller passing both means the control has nothing to do, not that
    // something is in flight — so it is really disabled, not merely busy.
    rerender(
      <Button disabled loading>
        Save
      </Button>,
    );
    expect(screen.getByRole("button")).toBeDisabled();
    expect(screen.getByRole("button")).not.toHaveAttribute("aria-disabled");
  });

  it("passes a caller's handler through when it is not busy", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Save</Button>);

    await user.click(screen.getByRole("button"));

    expect(onClick).toHaveBeenCalledTimes(1);
  });

  /*
   * jsdom applies no stylesheet, so whether the press actually stops is not
   * assertable here — but the reason it once did NOT stop is, and it is a
   * property of the class strings alone. `:not()` takes its argument's
   * specificity, so the guarded press weighs (0,4,0) and a bare
   * `motion-reduce:active:scale-100` weighs (0,2,0) and loses on specificity
   * whatever order Tailwind emits it in. The swap has to repeat both guards.
   * The failure is silent in every tier above this one.
   */
  it("guards the reduced-motion swap exactly as it guards the press", () => {
    render(<Button>Save</Button>);
    const classes = screen.getByRole("button").className.split(/\s+/);

    const press = classes.find((name) => name.endsWith("active:scale-97"));
    const stillness = classes.find((name) => name.endsWith("active:scale-100"));
    expect(press).toBeDefined();
    expect(stillness).toBeDefined();

    // Same selector, one prefixed by the variant that turns motion off.
    expect(stillness).toBe(`motion-reduce:${press?.replace("scale-97", "scale-100")}`);
  });
});
