import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ModelDownloadNudge } from "./ModelDownloadNudge";
import { ModelStatusProvider } from "../providers/ModelStatusProvider";
import { MODELS_STATE_EVENT } from "../../events";
import {
  emitFromBackend,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

const MISSING = {
  ready: false,
  bytesRequired: 762_000_000,
  bytesPresent: 0,
  sets: [],
  downloading: false,
  modelsDir: "C:\\app\\.models",
};

const READY = { ...MISSING, ready: true, bytesRequired: 0 };

function renderNudge(status: unknown = MISSING, onClose = () => {}) {
  onCommand("model_status", () => status);
  return render(
    <ModelStatusProvider>
      <ModelDownloadNudge onClose={onClose} />
    </ModelStatusProvider>,
  );
}

/** Pretend Rust reported download progress. */
function progress(overallReceived: number) {
  act(() => {
    emitFromBackend(MODELS_STATE_EVENT, {
      status: "downloading",
      file: "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx",
      file_index: 0,
      file_count: 5,
      file_received: overallReceived,
      file_total: 652_000_000,
      overall_received: overallReceived,
      overall_total: 762_000_000,
    });
  });
}

describe("ModelDownloadNudge", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("asks for the download, quoting the size before the user commits to it", async () => {
    renderNudge();
    expect(await screen.findByRole("button", { name: "Download 762 MB" })).toBeInTheDocument();
    expect(screen.getByText(/one time download of about 762 MB/)).toBeInTheDocument();
  });

  it("names the speech model's licence where the user is deciding", async () => {
    renderNudge();
    expect(await screen.findByText(/NVIDIA Parakeet, CC BY 4\.0/)).toBeInTheDocument();
  });

  it("shows nothing at all to a user whose models are already installed", async () => {
    const { container } = renderNudge(READY);
    await act(async () => {
      await Promise.resolve();
    });
    // Not merely "no dialog": a returning user must not get a confirmation
    // beat for a download they did not just watch.
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows nothing before the seed lands, so no ask flashes on every launch", () => {
    onCommand("model_status", () => MISSING);
    const { container } = render(
      <ModelStatusProvider>
        <ModelDownloadNudge onClose={() => {}} />
      </ModelStatusProvider>,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("starts the download on the button, never on its own", async () => {
    const user = userEvent.setup();
    renderNudge();
    onCommand("download_models", () => null);

    const button = await screen.findByRole("button", { name: "Download 762 MB" });
    expect(invokedCommands()).not.toContain("download_models");

    await user.click(button);
    expect(invokedCommands()).toContain("download_models");
  });

  it("reports real byte progress once the download runs", async () => {
    const user = userEvent.setup();
    renderNudge();
    onCommand("download_models", () => null);
    await user.click(await screen.findByRole("button", { name: "Download 762 MB" }));

    progress(300_000_000);
    expect(screen.getByText("300 MB of 762 MB")).toBeInTheDocument();
    expect(screen.getByText(/file 1 of 5/)).toBeInTheDocument();
  });

  it("says the recording is safe while the download runs", async () => {
    const user = userEvent.setup();
    renderNudge();
    onCommand("download_models", () => null);
    await user.click(await screen.findByRole("button", { name: "Download 762 MB" }));

    expect(screen.getByText(/Recordings you make now are saved/)).toBeInTheDocument();
  });

  it("offers to cancel a running download", async () => {
    const user = userEvent.setup();
    renderNudge();
    onCommand("download_models", () => null);
    onCommand("cancel_model_download", () => null);
    await user.click(await screen.findByRole("button", { name: "Download 762 MB" }));

    await user.click(screen.getByRole("button", { name: "Cancel download" }));
    expect(invokedCommands()).toContain("cancel_model_download");
  });

  it("keeps a failure up and offers to retry it", async () => {
    const user = userEvent.setup();
    renderNudge();
    onCommand("download_models", () => null);
    await user.click(await screen.findByRole("button", { name: "Download 762 MB" }));

    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, {
        status: "error",
        message: "no route to host",
      });
    });

    expect(screen.getByText(/no route to host/)).toBeInTheDocument();
    const retry = screen.getByRole("button", { name: "Try again" });
    await user.click(retry);
    expect(invokedCommands().filter((name) => name === "download_models")).toHaveLength(2);
  });

  it("confirms a download it watched finish, then closes itself", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const onClose = vi.fn();
    renderNudge(MISSING, onClose);
    onCommand("download_models", () => null);
    await user.click(await screen.findByRole("button", { name: "Download 762 MB" }));
    progress(762_000_000);

    onCommand("model_status", () => READY);
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, { status: "ready" });
    });

    await waitFor(() => expect(screen.getByText("Models ready.")).toBeInTheDocument());
    expect(onClose).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(2500);
    });
    expect(onClose).toHaveBeenCalled();
  });

  it("closes without downloading when the ask is declined", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    renderNudge(MISSING, onClose);

    await user.click(await screen.findByRole("button", { name: "Not now" }));
    expect(onClose).toHaveBeenCalled();
    expect(invokedCommands()).not.toContain("download_models");
  });
});
