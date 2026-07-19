import { vi } from "vitest";

/*
 * The Tauri IPC boundary, faked.
 *
 * Frontend code reaches the backend through exactly two `@tauri-apps/api`
 * entry points: `invoke` from `/core` and `listen` from `/event`. This module
 * re-implements both, so a test can stand in for Rust by pointing those two
 * module specifiers here:
 *
 *   vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
 *   vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));
 *
 * Mocking at that boundary (rather than stubbing each hook) is what keeps the
 * tests honest: the component under test runs its real hooks, real state
 * machines, and real wire types, and only the process boundary is simulated.
 *
 * The `vi.mock` calls belong in each test file, not in the shared setup file —
 * vitest hoists them per module graph, and the `() => import(...)` factory form
 * is the one that stays hoist-safe.
 *
 * Because this module *replaces* `@tauri-apps/api/event`, no export here may
 * shadow a name that module really exports. That is why the test-side "pretend
 * Rust fired an event" helper is `emitFromBackend` and not `emit`: the real
 * `emit` pushes an event *to* Rust, and a component importing it would
 * otherwise get this local fan-out — resolving synchronously, looping straight
 * back to its own subscribers, and passing a test that proves nothing.
 */

/** Stands in for one `#[tauri::command]`. Return a value (or a promise) to
 * resolve the call; throw to reject it. */
type CommandHandler = (args: Record<string, unknown> | undefined) => unknown;

/** The shape `listen` hands its subscribers — a Tauri event object, whose
 * `payload` is what the Rust side emitted. */
type EventCallback = (event: { event: string; payload: unknown }) => void;

const commandHandlers = new Map<string, CommandHandler>();
const eventListeners = new Map<string, Set<EventCallback>>();

/** Routes `invoke("<command>")` to `handler` for the rest of the test. Calling
 * it again for the same command replaces the handler, which is how a test
 * changes what the backend returns between two reads (e.g. a list that shrinks
 * after a re-route). */
export function onCommand(command: string, handler: CommandHandler): void {
  commandHandlers.set(command, handler);
}

/**
 * The `@tauri-apps/api/core` `invoke` stand-in. An unrouted command rejects
 * with a named message rather than hanging or resolving undefined, so a test
 * that forgets a stub fails saying which one.
 */
export const invoke = vi.fn(
  (command: string, args?: Record<string, unknown>): Promise<unknown> => {
    const handler = commandHandlers.get(command);
    if (!handler) {
      return Promise.reject(`no test handler for command "${command}"`);
    }
    try {
      return Promise.resolve(handler(args));
    } catch (thrown) {
      return Promise.reject(thrown);
    }
  },
);

/**
 * The `@tauri-apps/api/event` `listen` stand-in. Registration is synchronous
 * (so `emitFromBackend` right after render reaches the subscriber), but the returned
 * promise still resolves to an unlisten function — the async gap real callers
 * guard against, and which several hooks here have explicit teardown logic for.
 */
export const listen = vi.fn(
  (event: string, callback: EventCallback): Promise<() => void> => {
    const callbacks = eventListeners.get(event) ?? new Set<EventCallback>();
    callbacks.add(callback);
    eventListeners.set(event, callbacks);
    return Promise.resolve(() => {
      callbacks.delete(callback);
    });
  },
);

/** Delivers a backend event to every live subscriber, as Rust emitting it
 * would. Deliberately *not* named `emit` — see the module doc. Wrap calls in
 * React's `act`: subscribers set state synchronously. */
export function emitFromBackend(event: string, payload?: unknown): void {
  for (const callback of [...(eventListeners.get(event) ?? [])]) {
    callback({ event, payload });
  }
}

/** Live subscriber count for `event` — lets a test assert that a hook actually
 * unlistened rather than merely stopping at a guard. */
export function listenerCount(event: string): number {
  return eventListeners.get(event)?.size ?? 0;
}

/** The commands `invoke` was asked for, in order. Lives here rather than in
 * each test file because it reads `invoke.mock`, which is this module's
 * business; assert ordering with `toEqual`, presence with `toContain`. */
export function invokedCommands(): string[] {
  return invoke.mock.calls.map(([command]) => command);
}

/** Clears routes, subscribers, and call history. Call from `beforeEach`: the
 * module is a per-file singleton, so state leaks between tests otherwise. */
export function resetTauriMocks(): void {
  commandHandlers.clear();
  eventListeners.clear();
  invoke.mockClear();
  listen.mockClear();
}
