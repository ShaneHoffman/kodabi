import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  runMicTest,
  setAppearance,
  setCaptureOverlay,
  setRetentionPolicy,
  THEME_OPTIONS,
  useSettings,
  type MicCheckResult,
  type OverlaySettings,
  type RetentionKind,
  type Theme,
} from "../../useSettings";
import { applyReduceMotion, readReduceMotion } from "../../reduceMotion";
import { INDEX_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { useTimeout } from "../../useTimeout";
import { Button } from "../ui/Button";
import { Select } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./SettingsView.css";

/** The capture toggle's global shortcut, mirroring `DEFAULT_TOGGLE_SHORTCUT`
 * in `src-tauri/src/capture_control.rs`. Rendered rather than editable: the
 * backend registers it at startup and offers no rebinding command yet, and a
 * field that silently fails to save is worse than a value that plainly is
 * what it is. */
const CAPTURE_SHORTCUT = "Ctrl + Shift + K";

const TABS = ["Privacy", "Appearance", "Capture"] as const;
type Tab = (typeof TABS)[number];

/** How long a "that worked" line stays on screen.
 *
 * Successes are announcements, not state: they report something that has just
 * happened, and a report that never clears reads as a label describing how
 * things are. Long enough to be read without hunting for it, short enough
 * that coming back to the tab later shows the panel rather than the news.
 * Failures are never on this timer — see the note at its call site. */
const CONFIRMATION_MS = 4000;

/** Ids wiring the tab rail to the pane it filters. Module-level constants
 * rather than `useId`: there is exactly one Settings view mounted at a time,
 * and a stable id is what lets the arrow-key handler focus a tab by name. */
const PANEL_ID = "settings-panel";
const tabId = (name: Tab) => `settings-tab-${name.toLowerCase()}`;

/** The `index:state` payload, mirroring the Rust `IndexStateEvent` tagged enum
 * in `src-tauri/src/index_state.rs`. */
type IndexStateEvent =
  | { status: "rebuilding" }
  | { status: "ready"; notes: number }
  | { status: "error"; message: string };

type RebuildStatus = { status: "idle" } | IndexStateEvent;

/**
 * One setting: its name on the left, its control right-aligned into the
 * shared column. Nearly everything on this screen is one of these, which is
 * what makes the column scannable — the eye runs down the controls, not the
 * prose.
 *
 * `nested` marks a row that depends on the one above it, inside a `Group`. It
 * indents the LABEL and nothing else: the row still spans --measure-setting,
 * so the 1fr label track absorbs the indent and the `auto` control track stays
 * flush to exactly the same right edge. The single scannable column survives
 * the hierarchy, which is the only reason the hierarchy is drawn this way.
 */
function Row({
  label,
  nested = false,
  children,
}: {
  label: string;
  nested?: boolean;
  children: ReactNode;
}) {
  return (
    <div className={`settings__row${nested ? " settings__row--nested" : ""}`}>
      <span className="text-label text-text-soft">{label}</span>
      <div className="settings__control">{children}</div>
    </div>
  );
}

/**
 * A cluster of settings that only reads correctly as one thing: an option and
 * the option it depends on, or two halves of one concept. `role="group"` plus a
 * name, so the relationship reaches a screen reader rather than living only in
 * the indent — the same shape the command palette gives its sections.
 *
 * Two shapes, and one question picks between them: DOES THE CONCEPT ALREADY OWN
 * A CONTROL?
 *
 *   It does — that row heads the group, and `hideLabel` keeps `label` as the
 *     accessible name alone (the same prop `Select` takes, for the same
 *     reason). Retention heads its own group; "Days" nests under it.
 *   It does not — the group draws `label` as a heading row and the rows under
 *     it are PEERS at one indent. "Capture pill" is a concept rather than a
 *     setting, and its two toggles are independent booleans: nesting one under
 *     the other would draw a dependency that is not there. An indent is a
 *     mapping claim, so it has to be true.
 *
 * Deliberately NOT a disclosure. An expander is a control, and this screen sits
 * exactly on the four-control ceiling (docs/UI_CONVENTIONS.md, "How much one
 * surface may hold"). A group adds rank without adding anything to press.
 *
 * The heading ranks up with weight and ink, never with size and never with a
 * fourth eyebrow tracking step (docs/DESIGN_SYSTEM.md §1).
 */
function Group({
  label,
  hideLabel = false,
  children,
}: {
  label: string;
  hideLabel?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="settings__group" role="group" aria-label={label}>
      {!hideLabel && (
        <p className="settings__groupLabel text-label font-medium text-text">{label}</p>
      )}
      {children}
    </div>
  );
}

/** The one short line a setting is allowed to explain itself with. Never a
 * paragraph: a config panel that argues with you is a config panel nobody
 * finishes reading. `nested` puts it at its row's indent — it belongs to the
 * line above it, so it travels with it. */
function SubLabel({
  nested = false,
  children,
}: {
  nested?: boolean;
  children: ReactNode;
}) {
  return (
    <p
      className={`settings__sublabel${
        nested ? " settings__sublabel--nested" : ""
      } text-cap text-text-faint`}
    >
      {children}
    </p>
  );
}

/** A boolean whose state is its geometry. `role="switch"` rather than a
 * checkbox: the platform semantics for "this takes effect immediately" differ
 * from "this is one of several things you are about to submit". */
function Toggle({
  label,
  checked,
  onChange,
  busy = false,
}: {
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  /**
   * A write is in flight. `aria-disabled` + `aria-busy`, never the native
   * `disabled` attribute: this control is the thing the user just pressed, so
   * it is the thing that holds focus, and `disabled` would blur it and reset
   * focus to <body> for the length of every save (docs/DESIGN_SYSTEM.md §6).
   * It stays focusable and declines its own activation instead.
   *
   * There is no genuine-disable case for a toggle here — every switch in
   * Settings is always meaningful — so `busy` is the only inert form it takes.
   */
  busy?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-disabled={busy || undefined}
      aria-busy={busy || undefined}
      onClick={() => {
        if (busy) return;
        onChange(!checked);
      }}
      className="settings__toggle ui-focus-ring aria-disabled:cursor-not-allowed"
    >
      <span className="settings__knob" />
    </button>
  );
}

/** The Capture tab's stored mic-test result, phrased for what it means for a
 * recording rather than just what was measured. */
function micCheckSummary(result: MicCheckResult): string {
  switch (result.outcome) {
    case "headphones":
      return "Headphones detected. Your microphone and speaker channels stay separate.";
    case "speakers":
      return `Speakers detected. Your microphone hears them (about ${result.echo_db.toFixed(1)} dB, ${result.delay_ms.toFixed(0)} ms delay). Kodabi removes this automatically when transcribing.`;
    case "mic_silent":
      return "No signal from your microphone. Check that it is connected and not muted.";
  }
}

/**
 * Rebuilds the note index from the files on disk. The index is a derived cache
 * the file watcher normally keeps live; this is the manual "reconstruct from
 * scratch" escape hatch. The command returns as soon as the job is queued, so
 * progress is driven entirely by the `index:state` event.
 */
function RebuildIndexControl() {
  const [state, setState] = useState<RebuildStatus>({ status: "idle" });
  useTauriEvent<IndexStateEvent>(INDEX_STATE_EVENT, (payload) => setState(payload));

  // A success that never clears stops being a report and becomes a label: the
  // old "Index rebuilt." sat under the button for the rest of the session, so
  // a user coming back to this tab an hour later read it as the current state
  // rather than as something that had just happened. The failure below is
  // deliberately NOT on a timer — an error the user may not have been looking
  // at has to stay until it is read (docs/DESIGN_SYSTEM.md §3).
  useTimeout(
    () => setState({ status: "idle" }),
    state.status === "ready" ? CONFIRMATION_MS : null,
  );

  const rebuild = async () => {
    setState({ status: "rebuilding" });
    try {
      await invoke("rebuild_index");
    } catch (err) {
      // A command-level failure (index unavailable this session) never emits an
      // event, so surface it here.
      setState({ status: "error", message: String(err) });
    }
  };

  return (
    <>
      <Row label="Search index">
        <Button
          onClick={rebuild}
          loading={state.status === "rebuilding"}
          loadingLabel="Rebuilding…"
          className="text-label"
        >
          Rebuild
        </Button>
      </Row>
      <SubLabel>
        Reconstructs the index from your note files. It reads them and writes
        nothing back, so nothing you have written can be lost.
      </SubLabel>
      {state.status === "ready" && (
        <StatusMessage variant="status" compact>
          Index rebuilt. {state.notes} {state.notes === 1 ? "note" : "notes"} indexed.
        </StatusMessage>
      )}
      {state.status === "error" && (
        <StatusMessage variant="error" compact>
          Couldn&apos;t rebuild the index: {state.message}
        </StatusMessage>
      )}
    </>
  );
}

/**
 * Settings — the app's CONFIG PANEL, and it announces that before a word is
 * read: a small title, no list anywhere, and a horizontal tab rail that
 * filters the page rather than navigating it.
 *
 * Every setting is one label-and-control row on a shared grid, so all the
 * controls line up into a single column you can run your eye down. Controls
 * are raised chips; read-only values stay plain text or a mono chip and take
 * no chevron, so "you can change this" and "this is how it is" are told apart
 * by shape rather than by being greyed out.
 *
 * Where a setting only means something given another one, the two sit in a
 * `Group`: the dependent row indents, and the indent comes out of the LABEL
 * column, so the control column never moves.
 */
export function SettingsView() {
  const { settings, error, setSettings } = useSettings();
  const [tab, setTab] = useState<Tab>("Privacy");

  // Raw input string so the field can be cleared mid-edit rather than snapping
  // to 0; `buildRetentionPolicy` parses and clamps it on apply.
  const [days, setDays] = useState(String(DEFAULT_KEEP_DAYS));
  const [saveError, setSaveError] = useState<string | null>(null);
  const [overlayError, setOverlayError] = useState<string | null>(null);
  // Which async write is in flight, so the control that started it can say so
  // rather than flipping instantly and silently reverting if it fails.
  const [savingRetention, setSavingRetention] = useState(false);
  const [savingOverlay, setSavingOverlay] = useState(false);
  const [appearanceError, setAppearanceError] = useState<string | null>(null);
  const [savingAppearance, setSavingAppearance] = useState(false);
  const [micTestError, setMicTestError] = useState<string | null>(null);
  const [runningMicTest, setRunningMicTest] = useState(false);
  // Set once the day field has been committed, so the save is visible: this
  // control persists on Enter or blur, and without an acknowledgement a
  // keyboard user has no signal that anything happened.
  const [daysSaved, setDaysSaved] = useState(false);
  // Bumped on every successful commit. `daysSaved` is a boolean, and Enter
  // then blur (or two edits inside the window) both call `setDaysSaved(true)`
  // while it is already true — which does not restart the clear timer, so the
  // second "Saved." would inherit the first's remaining time and vanish early.
  // This counter is the timer's `resetKey`: it changes on each save even when
  // the flag does not, which is exactly the case `useTimeout`'s resetKey is for.
  const [daysSavedTick, setDaysSavedTick] = useState(0);
  // Seeded from storage during render rather than an effect — it is a plain
  // synchronous read (src/reduceMotion.ts).
  const [reduceMotion, setReduceMotion] = useState(readReduceMotion);

  // Seed the day field from the stored policy the first time a keep_days value
  // is seen, so editing starts from the stored value rather than the
  // placeholder default. The once-only flag means a later `settings` change (an
  // apply() echoing its result back) never overwrites an edit the user is
  // still typing.
  const [seededDays, setSeededDays] = useState(false);
  if (!seededDays && settings?.retention.policy === "keep_days") {
    setSeededDays(true);
    setDays(String(settings.retention.days));
  }

  // Clears the day field's acknowledgement after a beat, for the same reason
  // the index rebuild's does: "Saved." that stays put has stopped reporting an
  // event and started describing a state. The three error lines on this screen
  // are deliberately not on a timer.
  useTimeout(
    () => setDaysSaved(false),
    daysSaved ? CONFIRMATION_MS : null,
    daysSavedTick,
  );

  const kind: RetentionKind = settings?.retention.policy ?? "keep_all";

  const runMicTestClick = async () => {
    setMicTestError(null);
    setRunningMicTest(true);
    try {
      setSettings(await runMicTest());
    } catch (err) {
      setMicTestError(String(err));
    } finally {
      setRunningMicTest(false);
    }
  };

  const apply = async (nextKind: RetentionKind, nextDays: number) => {
    setSaveError(null);
    setSavingRetention(true);
    try {
      const updated = await setRetentionPolicy(buildRetentionPolicy(nextKind, nextDays));
      setSettings(updated);
      setDaysSaved(true);
      setDaysSavedTick((tick) => tick + 1);
    } catch (err) {
      setSaveError(String(err));
      setDaysSaved(false);
    } finally {
      setSavingRetention(false);
    }
  };

  // Both flags travel together (the command takes the whole struct), so each
  // toggle sends the stored pair with its own field replaced. Its own error
  // slot, not the retention one: an error has to appear beside the control
  // that failed, or it reads as a failure of whatever it sits under. It now
  // renders inside the "Capture pill" group for that same reason — it used to
  // sit at the foot of the tab, under Echo check and Search index, which is
  // exactly the failure this comment warns about. One slot for both toggles is
  // right, not a shortcut: this command writes the whole struct and holds one
  // error, so the pair IS the unit that fails.
  const applyOverlay = async (change: Partial<OverlaySettings>) => {
    if (!settings) return;
    setOverlayError(null);
    setSavingOverlay(true);
    try {
      const updated = await setCaptureOverlay({ ...settings.overlay, ...change });
      setSettings(updated);
    } catch (err) {
      setOverlayError(String(err));
    } finally {
      setSavingOverlay(false);
    }
  };

  // The window does not re-theme from here: src/theme.ts listens for the
  // settings event and applies it, so all three webviews change together
  // rather than only the one holding this control.
  const applyAppearanceTheme = async (theme: Theme) => {
    setAppearanceError(null);
    setSavingAppearance(true);
    try {
      setSettings(await setAppearance({ theme }));
    } catch (err) {
      setAppearanceError(String(err));
    } finally {
      setSavingAppearance(false);
    }
  };

  return (
    <ViewFrame variant="panel" eyebrow="System" title="Settings">
      {/* The rail filters; it does not navigate. Real tab semantics, so a
          screen reader announces it as a filter rather than as a second set
          of destinations competing with the sidebar.

          "Real" now means the whole contract, not half of it. Before this the
          rail had role=tablist and role=tab and nothing else: no ids, no
          aria-controls, no tabpanel under it, and every tab its own tab stop
          with the arrow keys doing nothing. A tablist that Tab walks through
          one tab at a time is a tablist in name only — the pattern is a SINGLE
          tab stop that Arrow keys move within, which is also what makes it
          faster than the thing it was imitating. */}
      <div className="settings__rail">
        <div
          className="settings__tabs"
          role="tablist"
          aria-label="Settings categories"
          onKeyDown={(event) => {
            const step =
              event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
            let next: Tab | null = null;
            if (step !== 0) {
              // Wraps: a rail of three is short enough that running off the
              // end and stopping feels like a bug rather than a boundary.
              const at = TABS.indexOf(tab);
              next = TABS[(at + step + TABS.length) % TABS.length];
            } else if (event.key === "Home") next = TABS[0];
            else if (event.key === "End") next = TABS[TABS.length - 1];
            if (!next) return;
            event.preventDefault();
            setTab(next);
            document.getElementById(tabId(next))?.focus();
          }}
        >
          {TABS.map((name) => (
            <button
              key={name}
              id={tabId(name)}
              type="button"
              role="tab"
              aria-selected={tab === name}
              aria-controls={PANEL_ID}
              // Roving tabindex: the rail is one stop in the page's tab order
              // and the arrow keys move inside it.
              tabIndex={tab === name ? 0 : -1}
              onClick={() => setTab(name)}
              className={`settings__tab ui-focus-ring text-label font-semibold ${
                tab === name ? "text-text" : "text-text-soft"
              }`}
            >
              {name}
            </button>
          ))}
        </div>
        <hr className="settings__rule" />
      </div>

      {error && (
        <StatusMessage variant="error">Couldn&apos;t load settings: {error}</StatusMessage>
      )}

      {settings && (
        <div
          id={PANEL_ID}
          role="tabpanel"
          aria-labelledby={tabId(tab)}
          className="settings__rows mt-sm"
        >
          {tab === "Privacy" && (
            <>
              <Row label="Recording consent">
                {/* Read-only, so it stays plain text with a check glyph — no
                    chip, no chevron, nothing that invites a click.

                    THE CHECK IS GATED ON THE VALUE. It used to render
                    unconditionally, so a profile that had never been shown the
                    consent nudge got a tick beside the words "Not yet shown" —
                    a false affirmative on the one row in the app that reports
                    a privacy state. --text-soft rather than --text-faint for
                    the same reason: this is a fact the user may need to read,
                    not a caption (docs/DESIGN_SYSTEM.md §6). */}
                <span className="inline-flex items-center gap-2xs text-label text-text-soft">
                  {settings.consent_acknowledged && (
                    <svg width="13" height="13" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                      <path
                        d="M2.5 7.5l3 3 6-7"
                        stroke="currentColor"
                        strokeWidth="1.5"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                      />
                    </svg>
                  )}
                  {settings.consent_acknowledged
                    ? "Acknowledged"
                    : "Not shown yet. It appears before your first capture."}
                </span>
              </Row>

              {/* The policy owns a control, so its row heads the group and the
                  group's name is the accessible one alone. */}
              <Group hideLabel label="Retention">
                <Row label="Retention">
                  <Select
                    hideLabel
                    label="Retention"
                    value={kind}
                    onChange={(value) => apply(value as RetentionKind, Number(days))}
                    options={RETENTION_OPTIONS}
                    busy={savingRetention}
                  />
                </Row>
                {/* Explains the head row, so it sits at the head row's rank. */}
                <SubLabel>
                  Kodabi deletes raw transcripts and recordings past this age.
                  Your notes are never removed.
                </SubLabel>

                {/* "Days", not "Days to keep". The group is named Retention and
                    that name is what a screen reader announces on entry, so the
                    child only has to name what it ADDS. ConsentNudge keeps the
                    long form on purpose (see the note at its call site): that
                    dialog is a flat column of fields with no group to lean on,
                    so the field there has to carry the whole meaning itself. */}
                {kind === "keep_days" && (
                  <Row nested label="Days">
                    <input
                      type="number"
                      min={1}
                      value={days}
                      aria-label="Days"
                      className="settings__chip ui-focus-ring w-24 text-right text-label text-text"
                      onChange={(event) => {
                        setDays(event.target.value);
                        setDaysSaved(false);
                      }}
                      // Enter as well as blur. Blur-only made this the one setting
                      // a keyboard user could change without ever being told it
                      // saved, and without a way to commit it deliberately
                      // (docs/DESIGN_SYSTEM.md §6).
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void apply("keep_days", Number(days));
                        }
                      }}
                      onBlur={() => apply("keep_days", Number(days))}
                    />
                  </Row>
                )}
                {/* The DAY FIELD's acknowledgement, so it sits at the day
                    field's indent. The error below it is the GROUP's: the Select
                    and the field both call apply(), so either can raise it, and
                    it belongs at the rank the two of them share. */}
                {daysSaved && kind === "keep_days" && (
                  <StatusMessage
                    variant="status"
                    compact
                    className="settings__status--nested"
                  >
                    Saved.
                  </StatusMessage>
                )}
                {saveError && (
                  <StatusMessage variant="error" compact>
                    Couldn&apos;t save the retention policy: {saveError}
                  </StatusMessage>
                )}
              </Group>
            </>
          )}

          {tab === "Appearance" && (
            <>
              <Row label="Theme">
                <Select
                  hideLabel
                  label="Theme"
                  value={settings.appearance.theme}
                  onChange={(value) => applyAppearanceTheme(value as Theme)}
                  options={THEME_OPTIONS}
                  busy={savingAppearance}
                />
              </Row>
              {/* Only true on `system`, so only said there. It used to render
                  under every choice, telling a user who had just picked Light
                  that the app follows their OS. */}
              {settings.appearance.theme === "system" && (
                <SubLabel>Follows your OS light and dark setting.</SubLabel>
              )}

              <Row label="Reduce motion">
                <Toggle
                  label="Reduce motion"
                  checked={reduceMotion}
                  onChange={(next) => {
                    // Applied in the handler, not an effect: it happens
                    // because the user did something
                    // (.claude/rules/no-use-effect.md).
                    setReduceMotion(next);
                    applyReduceMotion(next);
                  }}
                />
              </Row>
              <SubLabel>
                Holds the listening glow and the caret still. Your OS setting
                already does this; here it applies to Kodabi alone.
              </SubLabel>

              {appearanceError && (
                <StatusMessage variant="error" compact>
                  Couldn&apos;t save the theme: {appearanceError}
                </StatusMessage>
              )}
            </>
          )}

          {tab === "Capture" && (
            <>
              <Row label="Global shortcut">
                {/* Mono, because it is a key sequence — the same voice the
                    palette hint and every path in the app uses. */}
                <span className="settings__chip font-mono text-action text-text">
                  {CAPTURE_SHORTCUT}
                </span>
              </Row>
              <SubLabel>Starts and stops a capture from anywhere.</SubLabel>

              {/* The pill is a concept and owns no control of its own, so the
                  group draws it as a heading and the two toggles under it are
                  PEERS. They are independent flags: nesting the auto one under
                  the manual one would claim turning the pill off silences the
                  auto pill too, and it does not.

                  Each toggle's accessible name is now its visible row label,
                  verbatim. It used to be neither: the row read "Capture pill"
                  while the switch answered to "Show the capture pill during
                  captures you start", so what you read was not what you could
                  say to a voice-control tool. The context those long names
                  carried is the group's name now, which is where it belongs. */}
              <Group label="Capture pill">
                {/* Under the heading, not under a row: it describes the pill,
                    which is what both toggles switch. It used to hang off the
                    manual toggle and explain the auto one by accident. */}
                <SubLabel>
                  Stays on top of full screen apps, so a recording is never invisible.
                </SubLabel>

                <Row nested label="Captures you start">
                  <Toggle
                    label="Captures you start"
                    checked={settings.overlay.manual_captures}
                    busy={savingOverlay}
                    onChange={(checked) => applyOverlay({ manual_captures: checked })}
                  />
                </Row>

                <Row nested label="Auto detected captures">
                  <Toggle
                    label="Auto detected captures"
                    checked={settings.overlay.auto_captures}
                    busy={savingOverlay}
                    onChange={(checked) => applyOverlay({ auto_captures: checked })}
                  />
                </Row>
                <SubLabel nested>
                  Kodabi does not detect meetings on its own yet, so this has nothing
                  to act on today.
                </SubLabel>

                {/* Moved here from the foot of the tab, where it sat below Echo
                    check and Search index and read as THEIR failure. One slot
                    for both toggles, at the group's own rank rather than
                    indented under one of the two — see applyOverlay. */}
                {overlayError && (
                  <StatusMessage variant="error" compact>
                    Couldn&apos;t save the capture pill setting: {overlayError}
                  </StatusMessage>
                )}
              </Group>

              <Row label="Echo check">
                <Button
                  onClick={runMicTestClick}
                  loading={runningMicTest}
                  loadingLabel="Testing…"
                  className="text-label"
                >
                  Run test
                </Button>
              </Row>
              <SubLabel>
                Plays a short tone through your speakers and listens for it on
                your microphone, so you can see whether the two stay separate.
              </SubLabel>
              {settings.mic_check && !runningMicTest && (
                <StatusMessage variant="status" compact>
                  {micCheckSummary(settings.mic_check)} Checked{" "}
                  {new Date(settings.mic_check.measured_at).toLocaleString()}.
                </StatusMessage>
              )}
              {micTestError && (
                <StatusMessage variant="error" compact>
                  Couldn&apos;t run the mic test: {micTestError}
                </StatusMessage>
              )}

              <RebuildIndexControl />
            </>
          )}
        </div>
      )}
    </ViewFrame>
  );
}
