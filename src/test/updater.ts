import { vi } from "vitest";

/**
 * Stand-ins for the three modules the update flow talks to that
 * `src/test/tauri.ts` does not cover: `@tauri-apps/plugin-updater`,
 * `@tauri-apps/plugin-process` and `@tauri-apps/api/app`.
 *
 * One module serves all three because their exports do not collide (`check`,
 * `relaunch`, `getVersion`), the same trick `src/test/tauri.ts` plays with
 * `core` and `event`. A test file mocks whichever it needs:
 *
 *     vi.mock("@tauri-apps/plugin-updater", () => import("./test/updater"));
 *     vi.mock("@tauri-apps/plugin-process", () => import("./test/updater"));
 *     vi.mock("@tauri-apps/api/app", () => import("./test/updater"));
 *
 * Those calls are hoisted per file and cannot live in the shared setup.
 *
 * The `Update` the real plugin hands back is a live resource handle, so tests
 * build their own plain object with the two methods the flow calls. That is
 * deliberately not a class: a test that wants `download` to emit three progress
 * events and then resolve should just write that.
 */
export type FakeUpdate = {
  version: string;
  body?: string;
  download: (onEvent: (event: DownloadEventLike) => void) => Promise<void>;
  install: () => Promise<void>;
};

/** The `DownloadEvent` shape from the plugin, restated so tests can build one
 * without importing the module they are mocking. */
export type DownloadEventLike =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

let availableUpdate: FakeUpdate | null = null;
let checkError: unknown = null;
let appVersion = "0.1.0";

/** What the next `check()` resolves to. `null` means "nothing newer". */
export function setAvailableUpdate(update: FakeUpdate | null): void {
  availableUpdate = update;
  checkError = null;
}

/** Makes the next `check()` reject, the way an offline machine does. */
export function failCheck(error: unknown): void {
  checkError = error;
}

/** What `getVersion()` reports as the running build. */
export function setAppVersion(version: string): void {
  appVersion = version;
}

export const check = vi.fn((): Promise<FakeUpdate | null> => {
  if (checkError !== null) return Promise.reject(checkError);
  return Promise.resolve(availableUpdate);
});

export const relaunch = vi.fn((): Promise<void> => Promise.resolve());

export const getVersion = vi.fn((): Promise<string> => Promise.resolve(appVersion));

/** Clears the staged update and the call history. Call from `beforeEach`: the
 * module is a per-file singleton, so state leaks between tests otherwise. */
export function resetUpdaterMocks(): void {
  availableUpdate = null;
  checkError = null;
  appVersion = "0.1.0";
  check.mockClear();
  relaunch.mockClear();
  getVersion.mockClear();
}
