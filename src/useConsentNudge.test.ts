import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CONSENT_REQUIRED_EVENT } from "./events";
import { useConsentNudge } from "./useConsentNudge";
import { emit, listenerCount, resetTauriMocks } from "./test/tauri";

vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));

describe("useConsentNudge", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("stays closed until the backend gates a capture", () => {
    const { result } = renderHook(() => useConsentNudge());

    // Push-only: nothing is seeded on mount, so an unprompted mount must not
    // put the nudge in front of the user.
    expect(result.current.open).toBe(false);

    act(() => {
      emit(CONSENT_REQUIRED_EVENT);
    });

    expect(result.current.open).toBe(true);
  });

  it("closes on closeNudge", () => {
    const { result } = renderHook(() => useConsentNudge());
    act(() => {
      emit(CONSENT_REQUIRED_EVENT);
    });

    act(() => {
      result.current.closeNudge();
    });

    expect(result.current.open).toBe(false);
  });

  it("unlistens when unmounted before the subscription resolves", async () => {
    const { unmount } = renderHook(() => useConsentNudge());

    unmount();
    await act(async () => {});

    expect(listenerCount(CONSENT_REQUIRED_EVENT)).toBe(0);
  });
});
