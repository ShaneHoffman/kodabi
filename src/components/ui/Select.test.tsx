import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Select } from "./Select";

const OPTIONS = [
  { value: "keep_all", label: "Keep everything" },
  { value: "keep_days", label: "Keep for a number of days" },
];

/*
 * Five rows, and both the labels and the seeded value are load-bearing.
 *
 * "Briarwood"/"Brightside" share a prefix, so one keystroke is ambiguous and
 * several disambiguate — that is what can tell an accumulating typeahead buffer
 * apart from one that only looks at the latest character.
 *
 * The seeded value is the middle row, so a clamp at either end lands on a
 * different row than a wrap would. With two options, "stopped at the end" and
 * "wrapped around" are frequently the same index, and the assertion proves
 * nothing.
 */
const PROJECTS = [
  { value: "p_alder", label: "Alder Grove" },
  { value: "p_briarwood", label: "Briarwood Golf" },
  { value: "p_brightside", label: "Brightside Media" },
  { value: "p_cedar", label: "Cedar Point" },
  { value: "p_dover", label: "Dover Lane" },
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

/**
 * The keyboard fixture: five projects, the middle one chosen, trigger already
 * focused so a test can go straight to the keys.
 */
function renderProjects(props: Partial<Parameters<typeof Select>[0]> = {}) {
  const onChange = vi.fn();
  render(
    <Select
      label="Project"
      value="p_brightside"
      onChange={onChange}
      options={PROJECTS}
      {...props}
    />,
  );
  const trigger = screen.getByRole("combobox");
  trigger.focus();
  return { onChange, trigger };
}

/*
 * Assert the highlight, both halves at once: the id assistive tech follows and
 * the class the eye follows. Checking only one lets a half-moved highlight pass.
 *
 * The ids come from React's useId and are deliberately never written out here —
 * they are opaque and React-version-shaped ("«r0»" under 19). Read the row's own
 * id off the DOM and compare, the way CommandPalette.test.tsx does.
 */
function expectActive(trigger: HTMLElement, label: string): void {
  const row = screen.getByRole("option", { name: label });
  expect(trigger).toHaveAttribute("aria-activedescendant", row.id);
  expect(row).toHaveClass("is-active");
}

// One test spies on Element.prototype.scrollIntoView, which is a shared
// prototype the setup file has already patched. Restore it so the spy cannot
// outlive its test.
afterEach(() => {
  vi.restoreAllMocks();
});

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

  describe("opening", () => {
    it("opens on ArrowDown, ArrowUp, Enter and Space", async () => {
      // All four are activation keys for a collapsed listbox. Space is the
      // literal " " to user-event; "{Space}" is not a descriptor it knows.
      const user = userEvent.setup();

      for (const key of ["{ArrowDown}", "{ArrowUp}", "{Enter}", " "]) {
        const { unmount } = render(
          <Select label="Project" value="p_brightside" onChange={vi.fn()} options={PROJECTS} />,
        );
        screen.getByRole("combobox").focus();

        await user.keyboard(key);

        expect(screen.getByRole("listbox")).toBeInTheDocument();
        unmount();
      }
    });

    it("opens with the highlight on the chosen row rather than the first", async () => {
      // ArrowDown opens the list; it must not also advance it. The list should
      // come up showing the user where they already are.
      const user = userEvent.setup();
      const { trigger } = renderProjects();

      await user.keyboard("{ArrowDown}");

      expectActive(trigger, "Brightside Media");
    });

    it("opens back on the chosen row, however the last visit ended", async () => {
      // Reopening is not a resume. A highlight left somewhere else two minutes
      // ago is not where the user's attention is.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");
      await user.keyboard("{End}");
      expectActive(trigger, "Dover Lane");

      await user.keyboard("{Escape}");
      await user.keyboard("{ArrowDown}");

      expectActive(trigger, "Brightside Media");
    });
  });

  describe("walking the list", () => {
    it("walks down and up one row at a time", async () => {
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{ArrowDown}");
      expectActive(trigger, "Cedar Point");

      await user.keyboard("{ArrowUp}");
      expectActive(trigger, "Brightside Media");
    });

    it("stops at the last row rather than wrapping to the first", async () => {
      // Clamp, not wrap. Opening lands on index 2 of 5, so four more presses
      // clamp to Dover Lane while a wrap would walk 3 → 4 → 0 → 1 and land on
      // Briarwood Golf. Different rows, so this can actually fail if the clamp
      // is ever swapped for a modulo.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{ArrowDown>4/}");

      expectActive(trigger, "Dover Lane");
    });

    it("stops at the first row rather than wrapping to the last", async () => {
      // The mirror image: four presses up clamp to Alder Grove, where a wrap
      // would walk 1 → 0 → 4 → 3 and land on Cedar Point.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{ArrowUp>4/}");

      expectActive(trigger, "Alder Grove");
    });

    it("jumps to the first row on Home and the last on End", async () => {
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{Home}");
      expectActive(trigger, "Alder Grove");

      await user.keyboard("{End}");
      expectActive(trigger, "Dover Lane");
    });

    it("keeps the highlighted row scrolled into view", async () => {
      // jsdom implements no layout, so this asserts that the call was made
      // against the right element with the right argument — not a measured
      // scroll position, which would be a test pretending to know something it
      // cannot. It is still the only thing that proves Select feeds the
      // *active* option's id to the hook rather than a stale one.
      const user = userEvent.setup();
      const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");
      const { trigger } = renderProjects();

      await user.keyboard("{ArrowDown}");
      await user.keyboard("{ArrowDown}");

      const row = screen.getByRole("option", { name: "Cedar Point" });
      const { contexts } = scrollIntoView.mock;
      expect(trigger).toHaveAttribute("aria-activedescendant", row.id);
      expect(scrollIntoView).toHaveBeenLastCalledWith({ block: "nearest" });
      expect(contexts[contexts.length - 1]).toBe(row);
    });
  });

  describe("choosing and dismissing", () => {
    it("chooses the highlighted row on Enter", async () => {
      const user = userEvent.setup();
      const { onChange } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{ArrowDown}");
      await user.keyboard("{Enter}");

      expect(onChange).toHaveBeenCalledWith("p_cedar");
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("chooses the highlighted row on Space", async () => {
      const user = userEvent.setup();
      const { onChange } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{ArrowUp}");
      await user.keyboard(" ");

      expect(onChange).toHaveBeenCalledWith("p_briarwood");
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("closes on Escape and leaves focus on the trigger", async () => {
      // The outcome that matters to a keyboard user: the list is gone and they
      // have not lost their place in the page.
      //
      // Note what this does and does not prove. Focus never leaves the trigger
      // while the list is open — that is the point of an active-descendant
      // listbox, and the option rows preventDefault their own pointerdown to
      // keep it that way — so the `triggerRef.focus()` inside `close()` is
      // belt-and-braces here rather than the thing under test. The falsifiable
      // half of that distinction is the outside-press test below, where focus
      // provably has moved elsewhere and must be left there.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{Escape}");

      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      expect(trigger).toHaveFocus();
    });

    it("keeps Escape from reaching an enclosing dialog", async () => {
      // Select is used inside dialogs that close on Escape. If the key bubbled,
      // one press would collapse the list and the dialog around it, and the
      // user would lose the form they were filling in.
      const user = userEvent.setup();
      const onKeyDown = vi.fn();
      render(
        <div onKeyDown={onKeyDown}>
          <Select label="Project" value="p_brightside" onChange={vi.fn()} options={PROJECTS} />
        </div>,
      );
      screen.getByRole("combobox").focus();
      await user.keyboard("{ArrowDown}");
      onKeyDown.mockClear();

      await user.keyboard("{Escape}");

      expect(onKeyDown).not.toHaveBeenCalled();
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("collapses the list on Tab but lets focus leave", async () => {
      // Tab is the one key the widget acts on without swallowing: it collapses
      // the list behind the user rather than trapping them in it.
      const user = userEvent.setup();
      render(
        <>
          <Select label="Project" value="p_brightside" onChange={vi.fn()} options={PROJECTS} />
          <button type="button">after</button>
        </>,
      );
      screen.getByRole("combobox").focus();
      await user.keyboard("{ArrowDown}");
      expect(screen.getByRole("listbox")).toBeInTheDocument();

      await user.tab();

      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "after" })).toHaveFocus();
    });

    it("closes on a pointer press outside itself", async () => {
      // The press is the intent, so the list collapses as the gesture starts
      // rather than waiting for the click to complete. A raw pointerdown is
      // dispatched here because that is exactly what the hook listens for.
      const user = userEvent.setup();
      renderProjects();
      await user.keyboard("{ArrowDown}");

      fireEvent.pointerDown(document.body);

      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("does not pull focus back when it dismisses", async () => {
      // Dismissing on an outside press uses setOpen(false), not close(), and the
      // difference is deliberate: the user pressed elsewhere because they want
      // to be elsewhere, so the list must not drag focus back to the trigger on
      // its way out.
      //
      // The trigger is blurred first, and that is not incidental — it is the
      // only state in which the two spellings differ observably. When the press
      // lands on something focusable, that element takes focus *after* the
      // pointerdown, so `close()` and `setOpen(false)` reach the same end state
      // and an assertion there would pass against either. Blurring first is what
      // makes this test able to fail.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.click(trigger);
      expect(screen.getByRole("listbox")).toBeInTheDocument();
      trigger.blur();

      fireEvent.pointerDown(document.body);

      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      expect(trigger).not.toHaveFocus();
    });
  });

  describe("typeahead", () => {
    /*
     * Real timers would make the 500ms idle either a sleep or a race, so the
     * buffer's expiry is driven explicitly.
     *
     * `shouldAdvanceTime` is not optional decoration: user-event awaits a
     * timer-backed delay between keystrokes, and under a frozen fake clock
     * `user.keyboard` never resolves — `advanceTimers` alone does not unstick it.
     * The auto-advance it enables is tied to real elapsed time, which would be a
     * problem if it could drift past the very window under test; measured, five
     * keystrokes move the fake clock 20ms against a 500ms buffer, so there is
     * ample margin. Tests that need the buffer to expire say so.
     */
    beforeEach(() => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it("jumps to the first row starting with the typed letter", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("c");

      expectActive(trigger, "Cedar Point");
    });

    it("accumulates keystrokes, so later letters narrow the earlier ones", async () => {
      // No label starts with "g", so a matcher that only looked at the latest
      // keystroke would still be sitting on Briarwood Golf. Reaching Brightside
      // Media takes a buffer that kept all four characters.
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("b");
      expectActive(trigger, "Briarwood Golf");

      await user.keyboard("rig");

      expectActive(trigger, "Brightside Media");
    });

    it("forgets the buffer after a short idle", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");
      await user.keyboard("br");
      expectActive(trigger, "Briarwood Golf");

      await act(async () => {
        vi.advanceTimersByTime(500); // the typeahead idle in Select.tsx
      });
      await user.keyboard("c");

      // Had the buffer survived, "brc" would match nothing and the highlight
      // would still be on Briarwood Golf.
      expectActive(trigger, "Cedar Point");
    });

    it("leaves the highlight where it is when nothing matches", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("z");
      expectActive(trigger, "Brightside Media");

      // The dead keystroke stays in the buffer rather than being dropped, so the
      // next letter cannot match either until the idle clears it. Recovering
      // from a typo is a pause, not a correction.
      await user.keyboard("c");
      expectActive(trigger, "Brightside Media");

      await act(async () => {
        vi.advanceTimersByTime(500); // the typeahead idle in Select.tsx
      });
      await user.keyboard("c");

      expectActive(trigger, "Cedar Point");
    });

    it("opens the list and jumps in one keystroke when closed", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { trigger } = renderProjects();

      await user.keyboard("c");

      expect(screen.getByRole("listbox")).toBeInTheDocument();
      expectActive(trigger, "Cedar Point");
    });
  });

  describe("nothing to choose", () => {
    const renderEmpty = () =>
      renderProjects({ options: [], value: null, emptyLabel: "No projects yet." });

    it("opens and says so rather than refusing to open", async () => {
      // Refusing to open left a trigger that swallowed every click and looked
      // broken. An empty list that explains itself is the smaller failure.
      const user = userEvent.setup();
      renderEmpty();

      await user.keyboard("{ArrowDown}");

      expect(screen.getByRole("listbox")).toBeInTheDocument();
      expect(screen.getByText("No projects yet.")).toBeInTheDocument();
    });

    it("does not offer the empty row as something to choose", async () => {
      // The empty row is deliberately not role=option: there is nothing to
      // choose, so it must not be announced as selectable, and there is no
      // active row for the trigger to name.
      const user = userEvent.setup();
      const { trigger } = renderEmpty();

      await user.keyboard("{ArrowDown}");

      expect(screen.queryAllByRole("option")).toHaveLength(0);
      expect(trigger).not.toHaveAttribute("aria-activedescendant");
    });

    it("leaves the arrow keys nothing to reach", async () => {
      const user = userEvent.setup();
      const { trigger } = renderEmpty();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{ArrowDown}{ArrowUp}{Home}{End}");

      expect(screen.getByRole("listbox")).toBeInTheDocument();
      expect(screen.queryAllByRole("option")).toHaveLength(0);
      expect(trigger).not.toHaveAttribute("aria-activedescendant");
    });

    it("closes on Enter without inventing a choice", async () => {
      const user = userEvent.setup();
      const { onChange } = renderEmpty();
      await user.keyboard("{ArrowDown}");

      await user.keyboard("{Enter}");

      expect(onChange).not.toHaveBeenCalled();
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });
  });

  describe("the pointer and the keyboard", () => {
    it("hands the highlight to the row under a moving pointer", async () => {
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      fireEvent.mouseMove(screen.getByRole("option", { name: "Dover Lane" }), {
        clientX: 40,
        clientY: 90,
      });

      expectActive(trigger, "Dover Lane");
    });

    it("ignores a repeat move that did not actually move", async () => {
      // Scrolling shifts rows under a stationary cursor and Chromium re-fires
      // the move event, so a move the user did not make would otherwise steal
      // the highlight out from under the arrow keys. Synthetic events here
      // because replaying identical coordinates is the whole test.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      await user.keyboard("{ArrowDown}");

      // Park the pointer on Dover Lane, then walk the keyboard away from it.
      fireEvent.mouseMove(screen.getByRole("option", { name: "Dover Lane" }), {
        clientX: 40,
        clientY: 90,
      });
      await user.keyboard("{Home}");
      expectActive(trigger, "Alder Grove");

      // The same coordinates arriving again is not a move, whatever row they
      // now land on.
      fireEvent.mouseMove(screen.getByRole("option", { name: "Cedar Point" }), {
        clientX: 40,
        clientY: 90,
      });

      expectActive(trigger, "Alder Grove");
    });
  });

  describe("mid-composition keys", () => {
    it("leaves keys pressed inside an IME composition to the composition", async () => {
      // Composing kana sends real keydowns whose keys belong to the composition,
      // not to the control underneath — Enter commits the candidate, Escape
      // cancels it. user-event cannot set `isComposing`, so a synthetic event is
      // the only route to this branch.
      const user = userEvent.setup();
      const { trigger } = renderProjects();

      fireEvent.keyDown(trigger, { key: "ArrowDown", isComposing: true });
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

      await user.keyboard("{ArrowDown}");
      expect(screen.getByRole("listbox")).toBeInTheDocument();

      fireEvent.keyDown(trigger, { key: "Escape", isComposing: true });

      expect(screen.getByRole("listbox")).toBeInTheDocument();
    });
  });

  describe("aria-activedescendant", () => {
    it("names no row while the list is collapsed", async () => {
      // A dangling reference to a row that is not in the DOM is a thing screen
      // readers can announce.
      const user = userEvent.setup();
      const { trigger } = renderProjects();
      expect(trigger).not.toHaveAttribute("aria-activedescendant");

      await user.keyboard("{ArrowDown}");
      expect(trigger).toHaveAttribute("aria-activedescendant");

      await user.keyboard("{Escape}");

      expect(trigger).not.toHaveAttribute("aria-activedescendant");
    });
  });
});
