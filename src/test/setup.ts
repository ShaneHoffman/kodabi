import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

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

// Testing Library only self-registers its cleanup when a global `afterEach`
// exists, and this suite runs with `globals: false` (explicit imports, per the
// repo's TypeScript style). Without this, rendered trees pile up across tests
// in a file and role queries start matching the previous test's DOM.
afterEach(() => {
  cleanup();
});
