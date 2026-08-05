import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ViewFrame } from "./ViewFrame";

/**
 * The variants exist so a user can tell a queue from a library from a config
 * panel before reading the heading. Since the Grove shell that is carried by
 * the STANCE — the class each variant puts on the section, which owns its
 * gutter, measure and density — and no longer by the title's size. These tests
 * hold both halves: the stance stays per-variant, and the head does not.
 */
describe("ViewFrame", () => {
  it("gives a queue the same head as every other view", () => {
    // The queue's one-line masthead is gone. It existed so a list of chores
    // would not read like a chapter opening, which the 26px step settles for
    // every view at once — and it cost the Inbox a real heading to navigate to.
    const { container } = render(
      <ViewFrame variant="queue" title="Inbox" summary="5 to file">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(container.querySelector(".view--queue")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Inbox", level: 2 })).toHaveClass(
      "text-[26px]",
      "font-semibold",
      "tracking-[-0.01em]",
    );
    // The separator went with the masthead: the summary is its own node on the
    // title's baseline, not a clause appended to the sentence.
    expect(screen.getByText("5 to file")).toHaveClass("font-data", "text-ink-dim");
  });

  it("opens a library at the same step, in the same face", () => {
    render(
      <ViewFrame variant="library" title="briarwood-golf" summary="12 notes">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(screen.getByRole("heading", { name: "briarwood-golf" })).toHaveClass(
      "text-[26px]",
      "font-semibold",
      "tracking-[-0.01em]",
    );
    // A count that changes under the user, so it holds its column.
    expect(screen.getByText("12 notes")).toHaveClass("font-data", "tabular-nums");
  });

  it("opens a config panel at that step too", () => {
    // The step no longer distinguishes a panel from a library: what does is the
    // stance class above and the density below.
    render(
      <ViewFrame variant="panel" title="Settings">
        <p>controls</p>
      </ViewFrame>,
    );

    expect(screen.getByRole("heading", { name: "Settings" })).toHaveClass(
      "text-[26px]",
      "font-semibold",
    );
  });

  it("names the landmark from `label` when the title is composed of elements", () => {
    // `landmarkName` only names a section from a plain string, so a view that
    // puts anything beside its name — ProjectView's folder-hue dot — would
    // otherwise render an unnamed region. `label` is what keeps it a landmark.
    render(
      <ViewFrame
        variant="library"
        title={
          <span>
            <span aria-hidden>dot</span>
            Briarwood Golf
          </span>
        }
        label="Briarwood Golf"
      >
        <p>rows</p>
      </ViewFrame>,
    );

    expect(screen.getByRole("region", { name: "Briarwood Golf" })).toBeInTheDocument();
  });

  it("still names the landmark from a plain-string title", () => {
    render(
      <ViewFrame variant="queue" title="Inbox">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(screen.getByRole("region", { name: "Inbox" })).toBeInTheDocument();
  });

  it("gives each variant its own stance class, so the gutters cannot merge", () => {
    // The per-view gutters and measures live in ViewFrame.css keyed off these
    // classes. If a variant stopped emitting its own, two view types would
    // silently start sharing a layout — the exact thing the redesign undid.
    for (const variant of ["queue", "library", "panel", "health", "doc", "search"] as const) {
      const { container, unmount } = render(
        <ViewFrame variant={variant} title="Title">
          <p>body</p>
        </ViewFrame>,
      );
      expect(container.querySelector(`.view--${variant}`)).toBeInTheDocument();
      unmount();
    }
  });

  it("refuses a summary on a variant that has no role for one", () => {
    // `panel`, `doc` and `search` supply their own header shape or have no
    // typographic role for a summary, so one passed there has nowhere to sit.
    // It used to be accepted and silently dropped at render; it is now a type
    // error, which is the only version of this that a caller finds out about.
    // Children are supplied so that `summary` is the ONLY thing wrong here —
    // otherwise the expect-error would be satisfied by the missing prop and
    // stop proving anything.
    const rejected = (
      // @ts-expect-error `summary` is not part of the `doc` variant's props.
      <ViewFrame variant="doc" title="New note" summary="ignored">
        <p>form</p>
      </ViewFrame>
    );
    expect(rejected).toBeTruthy();

    // And the runtime still draws nothing for it, so an untyped caller (a
    // spread, a JS consumer) cannot get an unstyled line either.
    render(
      <ViewFrame variant="doc" title="New note">
        <p>form</p>
      </ViewFrame>,
    );
    expect(screen.queryByText("ignored")).not.toBeInTheDocument();
  });

  it("refuses a header action on a variant that draws no header", () => {
    // `doc` and `search` draw their own header, so a header-level action has
    // nowhere to sit: `renderHeader` returned before it ever reached one and
    // reported nothing. Same treatment as `summary` above — the caller finds
    // out at the call site instead of wondering where the button went.
    // Children are supplied so `action` is the ONLY thing wrong here.
    const rejected = (
      // @ts-expect-error `action` is not part of the `doc` variant's props.
      <ViewFrame variant="doc" action={<button type="button">Edit</button>}>
        <p>note</p>
      </ViewFrame>
    );
    expect(rejected).toBeTruthy();

    // And the render half, which is where the old silent drop happened: with
    // no `eyebrow` and no `title`, `renderHeader` returns before it reaches
    // the action, so nothing is drawn. Be precise about what that does NOT
    // lock: `action` has no variant check at all. The guard is
    // `!eyebrow && !title`, so an untyped caller that passed a title as well
    // WOULD get the action, in a header nobody designed. The type above is the
    // enforcement here, not this render (docs/UI_CONVENTIONS.md says the same).
    render(
      // @ts-expect-error same, for the render half.
      <ViewFrame variant="doc" action={<button type="button">Edit</button>}>
        <p>note</p>
      </ViewFrame>,
    );
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
  });
});
