/*
 * A controllable `window.matchMedia` for jsdom, which ships none at all.
 *
 * Two modules read a media query at import time and hold the result for the
 * life of the window — `theme.ts` resolves "system" through
 * `prefers-color-scheme`, and `contrast.ts` folds `prefers-contrast: more` into
 * the high-contrast class. Both capture the MediaQueryList object once, so a
 * test cannot reach it by re-stubbing later: the stub has to exist before the
 * module under test is imported, which is why `setup.ts` installs it globally
 * rather than each test file doing it for itself.
 *
 * The registry is what makes that workable. Each distinct query string gets one
 * persistent stub object, so the reference a module captured at import is the
 * same one `setMediaMatches` mutates and fires `change` on — which is exactly
 * how the real thing behaves when the OS theme flips at sunset.
 */

type Listener = (event: MediaQueryListEvent) => void;

type StubQuery = MediaQueryList & {
  matches: boolean;
  listeners: Set<Listener>;
};

const registry = new Map<string, StubQuery>();

function stubFor(query: string): StubQuery {
  const existing = registry.get(query);
  if (existing) return existing;

  const listeners = new Set<Listener>();
  const stub = {
    media: query,
    matches: false,
    listeners,
    onchange: null,
    addEventListener: (type: string, listener: Listener) => {
      if (type === "change") listeners.add(listener);
    },
    removeEventListener: (type: string, listener: Listener) => {
      if (type === "change") listeners.delete(listener);
    },
    // The legacy pair, kept because it is part of the interface and a caller
    // that reached for it should not silently no-op differently from the above.
    addListener: (listener: Listener) => listeners.add(listener),
    removeListener: (listener: Listener) => listeners.delete(listener),
    dispatchEvent: () => true,
  } as unknown as StubQuery;

  registry.set(query, stub);
  return stub;
}

/** Install the stub on `window`. Called once from `src/test/setup.ts`, before
 * any module that captures a query has been imported. */
export function installMatchMedia(): void {
  window.matchMedia = ((query: string) => stubFor(query)) as typeof window.matchMedia;
}

/**
 * Set whether a query matches, and notify anyone listening — the same order the
 * browser uses, so a listener that re-reads `.matches` sees the new value.
 */
export function setMediaMatches(query: string, matches: boolean): void {
  const stub = stubFor(query);
  stub.matches = matches;
  const event = { matches, media: query } as MediaQueryListEvent;
  for (const listener of stub.listeners) listener(event);
}

/** Reset every query to "does not match" and drop all listeners, so one test's
 * OS-preference story cannot leak into the next. The stub objects themselves
 * survive, because the modules under test are still holding them. */
export function resetMatchMedia(): void {
  for (const stub of registry.values()) {
    stub.matches = false;
    stub.listeners.clear();
  }
}
