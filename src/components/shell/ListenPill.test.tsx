import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ListenPill } from "./ListenPill";

/*
 * What this pins is the consent story, not the styling: which modes are
 * allowed to look like recording. The green and the clock are the two things
 * a user reads to decide whether a room is being captured, so a mode wandering
 * between tones is a correctness bug rather than a visual one.
 */

const pill = (container: HTMLElement) =>
  container.firstElementChild as HTMLElement;

const mark = (container: HTMLElement) =>
  container.querySelector(".spirit-mark") as HTMLElement;

describe("ListenPill", () => {
  it("wears the green and runs the clock while listening", () => {
    const { container } = render(
      <ListenPill mode="listening" label="Listening" elapsedSeconds={154} />,
    );

    expect(pill(container)).toHaveClass("bg-kodama/10", "border-kodama/30");
    expect(screen.getByRole("status")).toHaveTextContent("Listening");
    expect(screen.getByText("2:34")).toBeInTheDocument();
    expect(mark(container)).toHaveClass("is-listening");
  });

  it("stays green while degraded — one source is still recording", () => {
    const { container } = render(
      <ListenPill mode="degraded" label="Mic only" elapsedSeconds={154} />,
    );

    expect(pill(container)).toHaveClass("bg-kodama/10", "border-kodama/30");
    expect(screen.getByText("2:34")).toBeInTheDocument();
    expect(mark(container)).toHaveClass("is-degraded");
  });

  it("is neutral and clockless at idle", () => {
    const { container } = render(<ListenPill mode="idle" label="Not listening" />);

    expect(pill(container)).toHaveClass("bg-wash", "border-edge");
    expect(screen.getByRole("status")).toHaveClass("text-ink-dim");
    // Dormant, and provably not on air: the mark wears its idle class and none
    // of the classes that put green in the core.
    expect(mark(container)).toHaveClass("is-idle");
    expect(mark(container)).not.toHaveClass("is-listening", "is-degraded");
  });

  it.each([
    ["starting", "Starting"],
    ["reconnecting", "Reconnecting"],
  ] as const)("shows no clock while %s — nothing is being recorded yet", (mode, label) => {
    const { container } = render(
      // The elapsed value is deliberately supplied: the gate is the mode, not
      // the absence of a number, so a caller that keeps counting through a
      // reconnect still cannot make the pill claim it is recording.
      <ListenPill mode={mode} label={label} elapsedSeconds={154} />,
    );

    expect(pill(container)).toHaveClass("bg-wash");
    expect(screen.queryByText("2:34")).not.toBeInTheDocument();
    expect(mark(container)).toHaveClass(`is-${mode}`);
  });

  it("says why, when idle is a failure rather than a rest", () => {
    render(
      <ListenPill
        mode="idle"
        label="Not listening"
        detail="Capture failed to start"
      />,
    );

    // The label is the one `captureLabel` actually produces for this state, so
    // the assertion pins the sentence a screen reader really hears. In the same
    // live region as the state it explains: a bare "Not listening" is
    // indistinguishable from never having pressed anything, and no other
    // surface reports a start that captured nothing.
    expect(screen.getByRole("status")).toHaveTextContent(
      "Not listening Capture failed to start",
    );
  });

  it("drops the detail while live — the headline already carries it", () => {
    render(
      <ListenPill
        mode="degraded"
        label="Mic only"
        detail="System audio unavailable"
        elapsedSeconds={154}
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent("Mic only");
    expect(screen.queryByText("System audio unavailable")).not.toBeInTheDocument();
  });

  it("holds the label clear of the mark's aura", () => {
    const { container } = render(
      <ListenPill mode="listening" label="Listening" elapsedSeconds={154} />,
    );

    // Optical, not decorative: the mark's box is its core, the aura reaches
    // past it, and the gap against the clock has no such filler. Matches the
    // clearance CaptureOverlayPill gives its own mark, so the two pills read
    // the same. A static margin, so nothing shifts when the clock is hidden.
    expect(mark(container)).toHaveClass("mr-1");
  });

  it("morphs rather than snaps between the two tones", () => {
    const { container } = render(<ListenPill mode="idle" label="Not listening" />);

    expect(pill(container)).toHaveClass(
      "transition-[background-color,border-color,color]",
      "duration-300",
    );
  });
});
