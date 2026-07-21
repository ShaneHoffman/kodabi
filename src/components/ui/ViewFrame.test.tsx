import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ViewFrame } from "./ViewFrame";

/**
 * The variants exist so a user can tell a queue from a library from a config
 * panel before reading the heading. These tests hold the differences that
 * carry that: the stance class each variant puts on the section, the title's
 * size step, and the summary's typographic role.
 */
describe("ViewFrame", () => {
  it("folds a queue's workload into a one-line masthead", () => {
    // A queue has no big title at all: its name and the amount of work in it
    // are one sentence, because splitting them made a list of chores read like
    // a chapter opening.
    const { container } = render(
      <ViewFrame variant="queue" title="Inbox" summary="5 to file">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(container.querySelector(".view--queue")).toBeInTheDocument();
    expect(screen.queryByRole("heading")).not.toBeInTheDocument();
    expect(screen.getByText("Inbox")).toHaveClass("font-semibold");
    expect(screen.getByText("· 5 to file")).toHaveClass("text-text-faint");
  });

  it("opens a library at the largest title step", () => {
    render(
      <ViewFrame variant="library" title="paradise-golf" summary="12 notes">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(screen.getByRole("heading", { name: "paradise-golf" })).toHaveClass(
      "font-serif",
      "text-title-library",
    );
    // Quieter than a queue's on purpose: nothing in a library is waiting on you.
    expect(screen.getByText("12 notes")).toHaveClass("text-label", "text-text-faint");
  });

  it("opens a config panel two steps smaller than a library", () => {
    render(
      <ViewFrame variant="panel" title="Settings">
        <p>controls</p>
      </ViewFrame>,
    );

    expect(screen.getByRole("heading", { name: "Settings" })).toHaveClass(
      "text-title-panel",
    );
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
});
