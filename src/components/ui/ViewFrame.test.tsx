import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ViewFrame } from "./ViewFrame";

/**
 * The variants exist so a user can tell a queue from a library from a config
 * panel before reading the heading. These tests hold the two differences that
 * carry that: the summary's typographic role, and the panel's column.
 */
describe("ViewFrame", () => {
  it("gives a queue's summary the weight of a sentence about work", () => {
    render(
      <ViewFrame variant="queue" title="Inbox" summary="5 notes to file">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(screen.getByText("5 notes to file")).toHaveClass(
      "text-body",
      "text-text-soft",
    );
  });

  it("gives a library's summary the weight of a count", () => {
    // Quieter than a queue's on purpose: nothing in a library is waiting on you.
    render(
      <ViewFrame variant="library" title="Paradise Golf" summary="12 notes">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(screen.getByText("12 notes")).toHaveClass("text-cap", "text-text-faint");
  });

  it("runs a panel in the narrower reading measure", () => {
    const { container } = render(
      <ViewFrame variant="panel" title="Settings">
        <p>controls</p>
      </ViewFrame>,
    );

    expect(container.querySelector(".max-w-measure")).toBeInTheDocument();
    expect(container.querySelector(".max-w-content")).not.toBeInTheDocument();
  });

  it("keeps the content column for everything else", () => {
    const { container } = render(
      <ViewFrame variant="queue" title="Inbox">
        <p>rows</p>
      </ViewFrame>,
    );

    expect(container.querySelector(".max-w-content")).toBeInTheDocument();
  });

  it("renders no summary line without a variant to give it a role", () => {
    // The bare scaffold is a real fourth answer, not an oversight: the note
    // editor supplies its own header shape.
    render(
      <ViewFrame title="New note" summary="ignored">
        <p>form</p>
      </ViewFrame>,
    );

    expect(screen.queryByText("ignored")).not.toBeInTheDocument();
  });
});
