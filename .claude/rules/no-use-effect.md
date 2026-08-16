# No direct useEffect

An effect synchronizes React with **one** external system — a Tauri event, a DOM
listener, a timer. Left inline in components, that glue is where subscription
leaks, stale closures, and render-timing surprises live, and the frontend has no
tests to catch them. So every effect lives in a blessed, single-purpose *bridge
hook*; components and feature logic compose those hooks and never call
`useEffect` (or `useLayoutEffect`) themselves.

Enforced, not aspirational: `no-restricted-imports` plus `no-restricted-syntax`
in `eslint.config.js` fail any non-blessed file at the
`pnpm exec eslint . --max-warnings=0` gate.

- **Reach for the non-effect answer first.** Most "effects" aren't one. Derived
  data is computed during render (`useMemo` only when it's genuinely costly).
  Something that happens *because the user did something* belongs in the event
  handler. Resetting or pruning state when a prop changes is the
  adjust-state-during-render pattern — compare the previous prop to the current
  one in the render body and `setState` conditionally
  (`NeedsAttentionView`'s `rowErrors` prune and `SettingsView`'s one-time
  day-field seed are the in-repo precedents).
- **The blessed bridge hooks.** These files, and only these, may call
  `useEffect`. Backend events: [`useTauriEvent`](../../src/useTauriEvent.ts) (the
  `listen()` primitive every other subscription should prefer),
  `useVaultQuery` (vault fetch + refetch bus), `useCaptureState`,
  `useDistillState`, `useTranscriptionState`, `useSettings`, `useConsentNudge`,
  `useChatSession` (the chat view's backend session: `chat_open` on mount +
  the `chat:event` stream), `useRoutePreview` (the quick-capture footer's
  debounced routing-guess call), `useModelDownload` (first-run model
  provisioning: `model_status` on mount + the `models:state` stream),
  `useUpdater` (the release check on startup plus the download/install
  lifecycle of `@tauri-apps/plugin-updater`, and this build's own version),
  `useWindowMaximized` (the undecorated main window's maximize state, for the
  TopBar's caption glyph: an `isMaximized()` seed plus an `onResized`
  subscription that re-reads it),
  `useShortcutStatus` (whether the startup global-shortcut registrations bound:
  one `shortcut_status` read on mount, with no event to follow it because the
  backend records the outcome once and never revises it).
  DOM: `useCommandPalette` (global ⌘K/Ctrl-K keydown),
  `useScrollIntoView` (active-descendant row visibility),
  `useOutsidePointerDown` (dismiss on outside press), `useXterm` (the embedded
  terminal's xterm.js `Terminal`, streaming to the PTY, with the `ResizeObserver`
  that keeps the grid sized and the `MutationObserver` that re-reads the palette
  when the theme class changes).
  Timers:
  `useDebouncedValue`, `useTimeout`, `useElapsed` (the one-second
  recording clock). The list is duplicated as the override
  block in `eslint.config.js` — **the two must stay in lockstep.**
- **What qualifies as a bridge hook.** Named `useXxx`, flat in `src/`, owning
  exactly one external system. Cleanup is mandatory wherever there is anything
  to undo (unlisten, `clearTimeout`, remove the listener, restore focus). A
  changing handler is read through a ref assigned during render, so call sites
  can pass a fresh inline closure without re-subscribing — copy
  `useTauriEvent`'s shape.
- **Adding one is the escape hatch, and it is three edits in one change:** the
  hook file, its entry in the `eslint.config.js` override list, and its line in
  the list above. If the thing can't be described as one external system with
  its own cleanup, it isn't a bridge hook — it belongs inside an existing one,
  or it isn't an effect at all.
- **One-time DOM bootstrap is not an effect.** Document-level setup that runs
  once at startup (the theme class, the contrast and reduced-motion
  preferences) lives in `src/theme.ts`, `src/contrast.ts` and
  `src/reduceMotion.ts`, called imperatively from the entry modules
  (`src/main.tsx`, `src/capture.tsx`, `src/overlay.tsx`) — not from a component.
