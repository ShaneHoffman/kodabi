import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SpiritMark, type SpiritMarkMode } from "./SpiritMark";

/*
 * The mark is styled from the spirit-mark block in src/index.css §3, which
 * selects on the classes this component emits. jsdom loads no stylesheet, so
 * what is testable here is exactly that contract — the class names and the DOM
 * shape the CSS reaches for. Whether the green is the right green, or the
 * lobes settle round under reduced motion, is a thing you look at on
 * /gallery.html.
 *
 * It matters that this is pinned rather than assumed: the two halves live in
 * different files, and a renamed class breaks the mark silently — it still
 * renders, just inert and ink, which is the one failure mode a listening
 * indicator must never have.
 */

const MODES: SpiritMarkMode[] = [
  "idle",
  "starting",
  "listening",
  "degraded",
  "reconnecting",
];

const mark = (container: HTMLElement) =>
  container.querySelector(".spirit-mark") as HTMLElement;

describe("SpiritMark", () => {
  it.each([
    ["starting", "is-starting"],
    ["listening", "is-listening"],
    ["degraded", "is-degraded"],
    ["reconnecting", "is-reconnecting"],
  ] as const)("gives %s the %s class", (mode, expected) => {
    const { container } = render(<SpiritMark mode={mode} />);

    expect(mark(container)).toHaveClass("spirit-mark", expected);
  });

  it("gives idle no mode class — it is the resting state, not a state class", () => {
    const { container } = render(<SpiritMark mode="idle" />);

    expect(mark(container).className.trim()).toBe("spirit-mark");
  });

  it("renders the aura subtree and the core by default", () => {
    const { container } = render(<SpiritMark mode="listening" />);

    expect(
      container.querySelector(".spirit-mark__aura > .spirit-mark__bloom"),
    ).toBeInTheDocument();
    expect(container.querySelector(".spirit-mark__core")).toBeInTheDocument();
    expect(container.querySelector(".spirit-mark__ring")).not.toBeInTheDocument();
  });

  it("swaps the aura for the pulse ring in the ring variant", () => {
    const { container } = render(<SpiritMark mode="listening" variant="ring" />);

    expect(container.querySelector(".spirit-mark__ring")).toHaveClass(
      "animate-ring",
      // Movement stops, the breath does not (DESIGN_SYSTEM §4).
      "motion-reduce:animate-halo-still",
      // And the breath has to be visible to be a breath: the ring's box is
      // the core's box, so without a resting radius the opacity-only partner
      // pulses underneath an opaque disc and the reduced mark reads dead.
      "motion-reduce:scale-[1.7]",
    );
    expect(container.querySelector(".spirit-mark__aura")).not.toBeInTheDocument();
    expect(container.querySelector(".spirit-mark__core")).toBeInTheDocument();
  });

  it.each(MODES.filter((mode) => mode !== "listening"))(
    "pulses no ring in %s — the ring means audio is flowing",
    (mode) => {
      const { container } = render(<SpiritMark mode={mode} variant="ring" />);

      expect(container.querySelector(".spirit-mark__ring")).not.toBeInTheDocument();
    },
  );

  it("passes size and halo through as the custom properties the CSS reads", () => {
    const { container } = render(
      <SpiritMark mode="listening" size="14px" halo="11px" />,
    );

    expect(mark(container).style.getPropertyValue("--mark-size")).toBe("14px");
    expect(mark(container).style.getPropertyValue("--halo-spread")).toBe("11px");
  });

  it("is decorative — the state is carried by a text label beside it", () => {
    const { container } = render(<SpiritMark mode="listening" />);

    expect(mark(container)).toHaveAttribute("aria-hidden", "true");
  });
});
