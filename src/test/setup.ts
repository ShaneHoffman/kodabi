import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { installMatchMedia, resetMatchMedia } from "./media";

/*
 * Global test setup, loaded by `vite.config.ts`'s `test.setupFiles`.
 *
 * The jest-dom import above is doubly load-bearing: it registers the matchers
 * at runtime and augments vitest's `Assertion` interface for `tsc -b`, which
 * typechecks every file under `src/` — including the tests.
 */

// jsdom implements no layout, so `scrollIntoView` doesn't exist on Element at
// all. The Select primitive calls it on the active option whenever its listbox
// is open (src/components/ui/Select.tsx), which would throw the moment a test
// opens a picker. A no-op is the right stub: nothing here asserts on scrolling.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

// jsdom has no ResizeObserver either. `useXterm` observes its mount to keep the
// PTY grid matched to the viewport; a no-op stub lets the terminal view mount
// under test (the fit/resize is asserted at the IPC boundary, not via layout).
if (!("ResizeObserver" in globalThis)) {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
}

// jsdom has no `matchMedia`. Installed here rather than per-file because
// `theme.ts` and `contrast.ts` capture their queries at import time, so the stub
// has to predate the first import of either. See src/test/media.ts.
installMatchMedia();

// Testing Library only self-registers its cleanup when a global `afterEach`
// exists, and this suite runs with `globals: false` (explicit imports, per the
// repo's TypeScript style). Without this, rendered trees pile up across tests
// in a file and role queries start matching the previous test's DOM.
afterEach(() => {
  cleanup();
  // OS preferences are global state on `window`, so they leak between files
  // exactly the way a rendered tree does.
  resetMatchMedia();
});
