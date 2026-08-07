import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { UpdateNotice } from "./UpdateNotice";
import { UpdaterStatusContext, type UpdaterStatus } from "../../useUpdaterStatus";
import type { UpdaterPhase } from "../../useUpdater";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** The notice reads the context and nothing else, so a test drives it by
 * handing it a phase directly rather than by standing up the whole hook. */
function renderPhase(phase: UpdaterPhase, overrides: Partial<UpdaterStatus> = {}) {
  const value: UpdaterStatus = {
    state: { appVersion: "0.1.0", phase },
    check: vi.fn(async () => {}),
    download: vi.fn(async () => {}),
    install: vi.fn(async () => {}),
    ...overrides,
  };
  const onClose = vi.fn();
  render(
    <UpdaterStatusContext value={value}>
      <UpdateNotice onClose={onClose} />
    </UpdaterStatusContext>,
  );
  return { value, onClose };
}

describe("UpdateNotice", () => {
  it("says nothing before a check has found anything", () => {
    renderPhase({ status: "idle" });
    expect(screen.queryByTestId("update-notice")).not.toBeInTheDocument();
  });

  it("says nothing while a check is running", () => {
    renderPhase({ status: "checking" });
    expect(screen.queryByTestId("update-notice")).not.toBeInTheDocument();
  });

  it("says nothing when this build is current", () => {
    renderPhase({ status: "upToDate" });
    expect(screen.queryByTestId("update-notice")).not.toBeInTheDocument();
  });

  it("stays silent about a failed check, which the user never asked for", () => {
    // The Settings card is where a check failure is reported, because that is
    // the only place a human started one.
    renderPhase({ status: "error", step: "check", message: "no route to host" });
    expect(screen.queryByTestId("update-notice")).not.toBeInTheDocument();
  });

  it("names the waiting version and downloads only on a click", async () => {
    const user = userEvent.setup();
    const { value } = renderPhase({ status: "available", version: "0.2.0", notes: null });

    expect(screen.getByText(/Kodabi 0\.2\.0 is available\./)).toBeInTheDocument();
    expect(value.download).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(value.download).toHaveBeenCalled();
  });

  it("dismisses for the session without downloading", async () => {
    const user = userEvent.setup();
    const { value, onClose } = renderPhase({
      status: "available",
      version: "0.2.0",
      notes: null,
    });

    await user.click(screen.getByRole("button", { name: "Not now" }));
    expect(onClose).toHaveBeenCalled();
    expect(value.download).not.toHaveBeenCalled();
  });

  it("counts bytes against a total once the server has given one", () => {
    renderPhase({
      status: "downloading",
      version: "0.2.0",
      progress: { receivedBytes: 5_000_000, totalBytes: 20_000_000 },
    });
    expect(screen.getByText("5 MB of 20 MB")).toBeInTheDocument();
  });

  it("counts up alone when there is no content length to divide by", () => {
    renderPhase({
      status: "downloading",
      version: "0.2.0",
      progress: { receivedBytes: 5_000_000, totalBytes: null },
    });
    expect(screen.getByText("5 MB")).toBeInTheDocument();
  });

  it("warns that installing restarts, and installs only on a click", async () => {
    const user = userEvent.setup();
    const { value } = renderPhase({ status: "readyToInstall", version: "0.2.0" });

    expect(screen.getByText(/restarts the app/)).toBeInTheDocument();
    expect(value.install).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Restart and update" }));
    expect(value.install).toHaveBeenCalled();
  });

  it("reports a download failure as an alert, and says what is still fine", () => {
    renderPhase({ status: "error", step: "download", message: "the connection was reset" });
    const notice = screen.getByTestId("update-notice");
    expect(notice).toHaveAttribute("role", "alert");
    expect(screen.getByText(/the connection was reset/)).toBeInTheDocument();
    expect(screen.getByText(/your notes are safe/i)).toBeInTheDocument();
  });
});
