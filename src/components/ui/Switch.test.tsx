import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Switch } from "./Switch";

/*
 * jsdom applies no stylesheet, so nothing here can see the knob move — that is
 * a thing you look at, on /gallery.html, under all four grounds and with
 * Reduce motion on.
 *
 * What it CAN pin is the contract, and specifically the half that is easy to
 * undo by accident: a busy switch must stay focusable. The obvious "fix" for an
 * in-flight write is the native `disabled` attribute, and it silently breaks
 * the keyboard — an element that is focused when it becomes disabled is blurred
 * and focus resets to <body> (docs/DESIGN_SYSTEM.md §6). A regression to
 * `disabled` would look correct in every screenshot.
 */

function renderSwitch(props: Partial<Parameters<typeof Switch>[0]> = {}) {
  const onChange = vi.fn();
  const label = props.label ?? "Reduce motion";
  render(<Switch label={label} checked={false} onChange={onChange} {...props} />);
  // Found by the name the caller passed, which is the contract itself: there is
  // no other handle on this control.
  return { onChange, control: screen.getByRole("switch", { name: label }) };
}

describe("Switch", () => {
  it("reports its state through the switch role", () => {
    renderSwitch({ checked: true });

    expect(screen.getByRole("switch", { name: "Reduce motion" })).toBeChecked();
  });

  it("asks for the opposite of what it is showing", async () => {
    const user = userEvent.setup();
    const { onChange, control } = renderSwitch({ checked: true });

    await user.click(control);

    // It does not flip itself: the value is the caller's, so the caller
    // decides whether the write landed.
    expect(onChange).toHaveBeenCalledWith(false);
    expect(control).toBeChecked();
  });

  it("stays focusable while busy, and declines its own activation", async () => {
    const user = userEvent.setup();
    const { onChange, control } = renderSwitch({ busy: true });

    // aria-disabled says it went inert; aria-busy says why. Never the native
    // attribute, which is what would cost the keyboard its place in the page.
    expect(control).toHaveAttribute("aria-disabled", "true");
    expect(control).toHaveAttribute("aria-busy", "true");
    expect(control).not.toBeDisabled();

    control.focus();
    expect(control).toHaveFocus();

    await user.click(control);
    await user.keyboard(" ");

    expect(onChange).not.toHaveBeenCalled();
    // The point of all of the above: the user has not lost their place.
    expect(control).toHaveFocus();
  });

  it("takes its accessible name from the words printed beside it", () => {
    // The label is the whole name — there is no visible text inside the
    // control — so a voice-control user can say what they read.
    const { control } = renderSwitch({ label: "Pill for captures you start" });

    expect(control).toBeInTheDocument();
  });
});
