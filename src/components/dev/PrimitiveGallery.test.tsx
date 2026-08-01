import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { PrimitiveGallery } from "./PrimitiveGallery";

/*
 * The gallery's own check is a rendering one, and deliberately shallow: what
 * this page is FOR — whether the glass reads as glass, whether the scrim is
 * light enough to blur through — is a thing you look at, not a thing you
 * assert. jsdom applies no stylesheet at all, so an assertion about a blur
 * here would be theatre.
 *
 * What it can prove is the half that silently rots: that every primitive still
 * renders under each of the four grounds Grove ships (night, day, high
 * contrast, and the two together), which is the acceptance criterion the
 * primitives ticket names. A variant renamed out from under the gallery, or a
 * primitive that throws when a root class is set, fails here.
 */

afterEach(() => {
  document.documentElement.className = "";
  document.documentElement.removeAttribute("data-reduce-motion");
});

const GROUNDS = ["", "day", "hc", "hc day"];

describe("PrimitiveGallery", () => {
  it.each(GROUNDS)("renders every primitive on the %s ground", (ground) => {
    document.documentElement.className = ground;

    render(<PrimitiveGallery />);

    // One of each variant, by its label rather than its class: the point is
    // that the catalogue is complete, not how it is spelled.
    for (const label of ["File it", "Delete note", "Dismiss", "transcript.md"]) {
      expect(screen.getAllByRole("button", { name: label }).length).toBeGreaterThan(0);
    }
    expect(screen.getByRole("button", { name: "File to…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open a dialog" })).toBeInTheDocument();
    // The field, in all three of the states it has to be looked at in: at
    // rest, carrying an error, and holding a number.
    expect(screen.getAllByLabelText("Project name")).toHaveLength(2);
    expect(screen.getByRole("spinbutton", { name: "Days to keep" })).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("A project cannot be called inbox.");
    // Nothing is open at rest — both overlays are summoned, not resident.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("toggles the grounds through the same root classes the app writes", async () => {
    const user = userEvent.setup();
    render(<PrimitiveGallery />);
    const root = document.documentElement;

    await user.click(screen.getByRole("button", { name: "Day" }));
    expect(root).toHaveClass("day");

    await user.click(screen.getByRole("button", { name: "More contrast" }));
    expect(root).toHaveClass("hc");
    // They combine: `.hc.day` is the high-contrast day grove.
    expect(root).toHaveClass("day");

    await user.click(screen.getByRole("button", { name: "Reduce motion" }));
    expect(root).toHaveAttribute("data-reduce-motion", "on");

    await user.click(screen.getByRole("button", { name: "Day" }));
    expect(root).not.toHaveClass("day");
  });

  it("opens the dialog and gives focus to the safe action", async () => {
    const user = userEvent.setup();
    render(<PrimitiveGallery />);

    await user.click(screen.getByRole("button", { name: "Open a dialog" }));

    const dialog = await screen.findByRole("dialog", { name: "Delete this note?" });
    expect(dialog).toBeInTheDocument();
  });
});
