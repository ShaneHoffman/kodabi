import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppErrorBoundary } from "./AppErrorBoundary";

function Boom(): never {
  throw new Error("bad note shape");
}

describe("AppErrorBoundary", () => {
  beforeEach(() => {
    // React logs the caught error itself, and the boundary logs it again on
    // purpose (nothing else collects it). Neither is a test failure.
    vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("passes children through when nothing throws", () => {
    render(
      <AppErrorBoundary resetKey="inbox">
        <p>the inbox</p>
      </AppErrorBoundary>,
    );

    expect(screen.getByText("the inbox")).toBeInTheDocument();
  });

  it("shows a way forward instead of a blank window", () => {
    // Before the boundary, a throw anywhere in a view unmounted the whole app
    // and left an empty window with nothing to click.
    render(
      <AppErrorBoundary resetKey="inbox">
        <Boom />
      </AppErrorBoundary>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(/were not touched/);
    expect(screen.getByText(/Pick another screen/)).toBeInTheDocument();
    // The underlying message is kept, quietly: it is the only clue a user can
    // pass on, and hiding it helps nobody.
    expect(screen.getByText("bad note shape")).toBeInTheDocument();
  });

  it("clears once the user navigates somewhere else", () => {
    const { rerender } = render(
      <AppErrorBoundary resetKey="inbox">
        <Boom />
      </AppErrorBoundary>,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();

    rerender(
      <AppErrorBoundary resetKey="settings">
        <p>settings</p>
      </AppErrorBoundary>,
    );

    expect(screen.getByText("settings")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
