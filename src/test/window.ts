import { vi } from "vitest";

/**
 * A stand-in for `@tauri-apps/api/window`, the module the TopBar's caption
 * buttons talk to. Neither `src/test/tauri.ts` nor `src/test/updater.ts` covers
 * it: the window controls are Tauri's own `core:window` channels, reached
 * through a handle rather than through `invoke`. A test file mocks it the same
 * way:
 *
 *     vi.mock("@tauri-apps/api/window", () => import("./test/window"));
 *
 * That call is hoisted per file and cannot live in the shared setup.
 *
 * The handle is a plain object rather than a fake `Window` class, for the same
 * reason `test/updater.ts` hands back a plain update: a test only ever needs
 * the four methods the bar calls.
 */
type ResizeHandler = () => void;

let maximized = false;
const resizeHandlers = new Set<ResizeHandler>();

/** Maximizes or restores the fake window, firing every live `onResized`
 * subscriber the way a real resize does. Wrap in React's `act`: the handler
 * leads to a state update. */
export function setMaximized(value: boolean): void {
  maximized = value;
  for (const handler of [...resizeHandlers]) handler();
}

export const minimize = vi.fn((): Promise<void> => Promise.resolve());

export const toggleMaximize = vi.fn((): Promise<void> => Promise.resolve());

export const close = vi.fn((): Promise<void> => Promise.resolve());

export const isMaximized = vi.fn((): Promise<boolean> => Promise.resolve(maximized));

/** Registration is synchronous, but the unlisten still arrives in a promise —
 * the async gap `useWindowMaximized` guards its unmount against. */
export const onResized = vi.fn((handler: ResizeHandler): Promise<() => void> => {
  resizeHandlers.add(handler);
  return Promise.resolve(() => {
    resizeHandlers.delete(handler);
  });
});

const fakeWindow = { minimize, toggleMaximize, close, isMaximized, onResized };

export const getCurrentWindow = vi.fn(() => fakeWindow);

/** Live `onResized` subscriber count — lets a test assert a hook actually
 * unlistened rather than merely stopping at a guard. Mirrors `listenerCount`
 * in `src/test/tauri.ts`. */
export function resizeListenerCount(): number {
  return resizeHandlers.size;
}

/** Clears window state, subscribers, and call history. Call from `beforeEach`:
 * the module is a per-file singleton, so state leaks between tests otherwise. */
export function resetWindowMocks(): void {
  maximized = false;
  resizeHandlers.clear();
  minimize.mockClear();
  toggleMaximize.mockClear();
  close.mockClear();
  isMaximized.mockClear();
  onResized.mockClear();
  getCurrentWindow.mockClear();
}
