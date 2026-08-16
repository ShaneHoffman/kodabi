import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ModelDownloadNudge } from "./ModelDownloadNudge";
import { ModelStatusProvider } from "../providers/ModelStatusProvider";
import { MODELS_STATE_EVENT } from "../../events";
import { NavigationContext } from "../../useNavigation";
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
  // The ready beat offers the vault glossary, so the nudge reads navigation.
  const navigate = vi.fn();
  return {
    navigate,
    ...render(
      <NavigationContext.Provider value={{ view: { kind: "inbox" }, navigate }}>
        <ModelStatusProvider>
          <ModelDownloadNudge onClose={onClose} />
        </ModelStatusProvider>
      </NavigationContext.Provider>,
    ),
  };
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
      <NavigationContext.Provider value={{ view: { kind: "inbox" }, navigate: vi.fn() }}>
        <ModelStatusProvider>
          <ModelDownloadNudge onClose={() => {}} />
        </ModelStatusProvider>
      </NavigationContext.Provider>,
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

  it("confirms a download it watched finish, and waits rather than clearing itself", async () => {
    // The beat used to close itself after 2.5s, which a bare confirmation may
    // do. This one is not bare: it carries the glossary ask, so vanishing as
    // the user reaches for it would be worse than waiting to be dismissed.
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

    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText("Models ready.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("points a first-run user at the vault glossary once the models land", async () => {
    // The one onboarding moment for glossary seeding: transcription has just
    // become possible and nothing has been recorded yet, so the terms it should
    // spell right can still be seeded ahead of the first meeting.
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { navigate } = renderNudge(MISSING, onClose);
    onCommand("download_models", () => null);
    await user.click(await screen.findByRole("button", { name: "Download 762 MB" }));

    onCommand("model_status", () => READY);
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, { status: "ready" });
    });

    const seed = await screen.findByRole("button", { name: "Add glossary terms" });
    expect(screen.getByText(/vault glossary/)).toBeInTheDocument();

    await user.click(seed);
    // Vault-wide (slug null), not a project's: this is the glossary every
    // capture is transcribed against.
    expect(navigate).toHaveBeenCalledWith({ kind: "glossary", slug: null });
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
