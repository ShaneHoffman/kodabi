import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Select } from "./Select";

const OPTIONS = [
  { value: "keep_all", label: "Keep everything" },
  { value: "keep_days", label: "Keep for a number of days" },
];

function renderSelect(props: Partial<Parameters<typeof Select>[0]> = {}) {
  const onChange = vi.fn();
  render(
    <Select
      label="Retention"
      value="keep_all"
      onChange={onChange}
      options={OPTIONS}
      {...props}
    />,
  );
  return { onChange, trigger: screen.getByRole("combobox") };
}

describe("Select", () => {
  describe("busy — a write in flight", () => {
    // docs/DESIGN_SYSTEM.md §6. The native `disabled` attribute blurs a focused
    // element and focus resets to <body> (the HTML focus fixup rule), so every
    // keyboard user lost their place in the page for the length of each save —
    // and inside a modal it also stranded them, because the dialog's Escape and
    // Tab handling lives on an ancestor the focus had just left.

    it("does not acquire the native attribute when the write starts", async () => {
      // The mechanism, asserted where it can be: focus the trigger, then flip
      // it inert underneath the user and check it never picks up `disabled`.
      //
      // Deliberately NOT asserting that focus survives. In a real browser the
      // HTML focus fixup rule blurs a focused element the moment it becomes
      // disabled, which is the entire bug — but jsdom does not implement that
      // rule, so `toHaveFocus()` passes here either way and would be a test
      // that proves nothing. The absence of the attribute is the falsifiable
      // half; the blur itself is checked by hand in a real window.
      const user = userEvent.setup();
      const props = { label: "Retention", value: "keep_all", options: OPTIONS };
      const { rerender } = render(<Select {...props} onChange={vi.fn()} />);

      await user.tab();
      const trigger = screen.getByRole("combobox");
      expect(trigger).toHaveFocus();

      rerender(<Select {...props} onChange={vi.fn()} busy />);

      expect(trigger).not.toBeDisabled();
      expect(trigger).toHaveAttribute("aria-busy", "true");
    });

    it("marks itself inert to assistive tech without the native attribute", () => {
      renderSelect({ busy: true });
      const trigger = screen.getByRole("combobox");

      expect(trigger).toHaveAttribute("aria-disabled", "true");
      expect(trigger).toHaveAttribute("aria-busy", "true");
      expect(trigger).not.toBeDisabled();
    });

    it("swallows its own activation while busy", async () => {
      // A focusable control still receives clicks and keys, so unlike a
      // natively disabled one it has to decline them itself. (This assertion
      // alone would also pass against `disabled`; it is here to pin the
      // component's own guard, not to distinguish the two — the attribute
      // assertions above do that.)
      const user = userEvent.setup();
      const { trigger } = renderSelect({ busy: true });

      await user.click(trigger);
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

      trigger.focus();
      await user.keyboard("{ArrowDown}");
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("lets Tab through, so focus can still leave", async () => {
      const user = userEvent.setup();
      render(
        <>
          <Select label="Retention" value="keep_all" onChange={vi.fn()} options={OPTIONS} busy />
          <button type="button">after</button>
        </>,
      );

      screen.getByRole("combobox").focus();
      await user.tab();

      expect(screen.getByRole("button", { name: "after" })).toHaveFocus();
    });

    it("gives an explicit `disabled` precedence, native attribute and all", () => {
      // A caller asking for a genuinely inert control means it; passing both
      // for the same condition puts the focus loss back, on purpose, so the
      // two props never silently blend.
      renderSelect({ busy: true, disabled: true });
      const trigger = screen.getByRole("combobox");

      expect(trigger).toBeDisabled();
      expect(trigger).not.toHaveAttribute("aria-disabled");
    });
  });

  it("still opens and chooses when it is not busy", async () => {
    const user = userEvent.setup();
    const { onChange, trigger } = renderSelect();

    await user.click(trigger);
    await user.click(screen.getByRole("option", { name: /number of days/ }));

    expect(onChange).toHaveBeenCalledWith("keep_days");
  });
});
