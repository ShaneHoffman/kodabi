import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { backendCopy } from "../../errorCopy";
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
import { useNavigation } from "../../useNavigation";
import { applyContrast, readContrast } from "../../contrast";
import { applyReduceMotion, readReduceMotion } from "../../reduceMotion";
import { INDEX_STATE_EVENT } from "../../events";
import { formatMegabytes } from "../../models";
import { useModelStatus } from "../../useModelStatus";
import { useUpdaterStatus } from "../../useUpdaterStatus";
import { useTauriEvent } from "../../useTauriEvent";
import { useTimeout } from "../../useTimeout";
import { ModelDownloadProgress } from "../models/ModelDownloadProgress";
import { Button } from "../ui/Button";
import { Select } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { Switch } from "../ui/Switch";
import { ViewFrame } from "../ui/ViewFrame";

/** The capture toggle's global shortcut, mirroring `DEFAULT_TOGGLE_SHORTCUT`
 * in `src-tauri/src/capture_control.rs`. Rendered rather than editable: the
 * backend registers it at startup and offers no rebinding command yet, and a
 * field that silently fails to save is worse than a value that plainly is
 * what it is. */
const CAPTURE_SHORTCUT = "Ctrl + Shift + K";

/** How long a "that worked" line stays on screen.
 *
 * Successes are announcements, not state: they report something that has just
 * happened, and a report that never clears reads as a label describing how
 * things are. Long enough to be read without hunting for it, short enough
 * that coming back to the screen later shows the panel rather than the news.
 * Failures are never on this timer — see the note at its call site. */
const CONFIRMATION_MS = 4000;

/** The `index:state` payload, mirroring the Rust `IndexStateEvent` tagged enum
 * in `src-tauri/src/index_state.rs`. */
type IndexStateEvent =
  | { status: "rebuilding" }
  | { status: "ready"; notes: number }
  | { status: "error"; message: string };

type RebuildStatus = { status: "idle" } | IndexStateEvent;

/**
 * One concern, as a card.
 *
 * They stack one per row down the panel, and the card is what replaced the tab
 * rail: a rail filtered the page, so three quarters of the settings were
 * somewhere you had to remember to look. The cards are the same
 * information with nothing hidden, and the eyebrow does the work the tab did —
 * naming the group — without also being a control.
 *
 * `<section aria-label>` rather than a bare div, so each card is a real region
 * landmark a screen reader can jump between. That is the whole hierarchy on
 * this screen now: the card names the concern, and every row inside it sits at
 * one rank. No indents anywhere.
 */
function Card({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section aria-label={title} className="glass-card flex flex-col px-5 pt-4 pb-1">
      {/* The one eyebrow recipe, verbatim: font-data at 10px on wide tracking.
          Never a second level of it on one surface (docs/DESIGN_SYSTEM.md §1). */}
      <h3 className="font-data text-[10px] tracking-[0.22em] text-ink-faint uppercase">
        {title}
      </h3>
      <div className="mt-1 flex flex-col">{children}</div>
    </section>
  );
}

/**
 * One setting: its name and its one line of explanation stacked on the left,
 * its control flush right. Every control on the page lands on the same right
 * edge, which is what makes the column scannable — the eye runs down the
 * controls, not the prose. The card runs the panel's full width, so that edge
 * is the panel's; what keeps the row readable at any width is the hint's own
 * measure on the left, not a cap on the card.
 *
 * THERE IS NO INDENT, and nothing on this screen needs one. A dependent control
 * sits inline beside the control it depends on (the day field beside the
 * retention policy), and two independent switches say what they are in their
 * own labels rather than borrowing meaning from a group heading above them. An
 * indent is a claim about what a line belongs to, and every claim this screen
 * used to make that way is now made by position or by words.
 *
 * `foot` is the row's own status and error lines. They sit INSIDE the row's
 * hairline unit rather than after it, so an error belongs visibly to the
 * control that raised it rather than to whatever follows.
 */
function Row({
  label,
  hint,
  foot,
  children,
}: {
  label: string;
  hint?: ReactNode;
  foot?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="border-t border-edge py-3 first:border-t-0">
      <div className="flex items-center justify-between gap-5">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="text-[13.5px] font-semibold text-ink">{label}</span>
          {/* The one short line a setting is allowed to explain itself with,
              capped at the measure a hint gets (docs/DESIGN_SYSTEM.md §1).
              Never a paragraph: a config panel that argues with you is a config
              panel nobody finishes reading. */}
          {hint && (
            <p className="max-w-[46ch] text-[11.5px] leading-relaxed text-ink-faint">
              {hint}
            </p>
          )}
        </div>
        {children && <div className="flex flex-none items-center gap-2">{children}</div>}
      </div>
      {foot}
    </div>
  );
}

/** A row's "that worked" line: an ANNOUNCEMENT, not a label, which is why every
 * one of them is on a timer somewhere above. It is `StatusMessage`'s `status`
 * variant — the primitive owns the polite `role="status"`, which waits its turn
 * rather than interrupting because it accompanies something the user started —
 * wrapped here only to keep the four call sites on one measure and one offset. */
function ConfirmLine({ children }: { children: ReactNode }) {
  return (
    <StatusMessage variant="status" compact className="mt-1.5 max-w-[56ch]">
      {children}
    </StatusMessage>
  );
}

/** The Capture card's stored mic-test result, phrased for what it means for a
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
 * The way into the vault-wide glossary, which has no other home: every project
 * glossary is reachable from its own project, but the vault one belongs to no
 * folder — and it is the one that matters most, since transcription biases
 * against it for every capture (a session is transcribed before routing has
 * picked a project).
 *
 * A link, not an editor. Settings rows are label-left / control-right and hold
 * one value each; a list you add to and delete from is a view, so this row
 * hands off to one rather than growing a second layout inside the card.
 */
function GlossaryControl() {
  const { navigate } = useNavigation();

  return (
    <Row
      label="Vault glossary"
      hint="Terms that bias transcription for every capture, like product names and coworkers. Each project keeps its own glossary for routing, edited from that project."
    >
      <Button onClick={() => navigate({ kind: "glossary", slug: null })}>Manage</Button>
    </Row>
  );
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
  // a user coming back an hour later read it as the current state rather than
  // as something that had just happened. The failure below is deliberately NOT
  // on a timer — an error the user may not have been looking at has to stay
  // until it is read (docs/DESIGN_SYSTEM.md §3).
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
      setState({
        status: "error",
        message: backendCopy(
          err,
          "Couldn't start the rebuild. Search may be stale but your notes are unaffected; try again.",
        ),
      });
    }
  };

  return (
    <Row
      label="Search index"
      hint="Reconstructs the index from your note files. It reads them and writes nothing back, so nothing you have written can be lost."
      foot={
        <>
          {state.status === "ready" && (
            <ConfirmLine>
              Index rebuilt. {state.notes} {state.notes === 1 ? "note" : "notes"} indexed.
            </ConfirmLine>
          )}
          {state.status === "error" && (
            <StatusMessage variant="error" compact>
              {state.message}
            </StatusMessage>
          )}
        </>
      }
    >
      <Button
        onClick={rebuild}
        loading={state.status === "rebuilding"}
        loadingLabel="Rebuilding…"
      >
        Rebuild
      </Button>
    </Row>
  );
}

/**
 * The speech and search models, which are downloaded on first run rather than
 * shipped in the installer. This is the permanent path to that download: the
 * first-run nudge is dismissible and session-only, so if it is waved away this
 * row is where the user finds it again, and where a failed attempt is retried.
 *
 * "Installed" carries no timer, unlike the rebuild confirmation above it. That
 * one reports something that just happened; this one states how the machine is
 * configured, which stays true.
 */
function ModelsControl() {
  const { state, start, cancel } = useModelStatus();

  return (
    <Row
      label="Speech and search models"
      hint="Transcription and note search run on this device. They need a one time download."
      foot={
        <>
          {state.status === "downloading" && (
            <ModelDownloadProgress progress={state.progress} />
          )}
          {state.status === "error" && (
            <StatusMessage variant="error" compact>
              {state.message}
            </StatusMessage>
          )}
        </>
      }
    >
      {state.status === "downloading" ? (
        <div className="flex items-center gap-2.5">
          <Button variant="quiet" onClick={() => void cancel()}>
            Cancel
          </Button>
          <Button loading loadingLabel="Downloading…">
            Downloading
          </Button>
        </div>
      ) : state.status === "ready" ? (
        // Read-only fact, so plain text and no control: "this is how it is"
        // must never look like "you can change this".
        <span className="text-[12.5px] text-ink-read">
          {state.envOverridden ? "Developer override" : "Installed"}
        </span>
      ) : state.status === "unknown" ? (
        <span className="text-[12.5px] text-ink-faint">Checking…</span>
      ) : (
        <Button onClick={() => void start()}>
          {state.status === "missing"
            ? `Download ${formatMegabytes(state.bytesRequired)}`
            : "Try again"}
        </Button>
      )}
    </Row>
  );
}

/**
 * The permanent path to an update, and the one that reports its failures.
 *
 * The corner notice is dismissible and session-only, so this is where a user
 * who waved it away finds the update again — and it is the only surface that
 * shows a *check* failing. A check the app ran unasked at launch stays quiet
 * (`useUpdater`); a check someone clicked has to answer.
 *
 * Every state here is one click from the last: check, download, restart. There
 * is no path through this row that updates anything without being told to.
 */
function UpdateControl() {
  const { state, check, download, install } = useUpdaterStatus();
  const { phase } = state;

  // Same split as the rebuild row above: "you are on the latest version" is a
  // report of something that just happened and clears itself, while a failure
  // stays until it has been read (docs/DESIGN_SYSTEM.md §3).
  //
  // Keyed to the CHECK, not to the phase, and that distinction is the whole
  // reason this is three pieces of state rather than a boolean. `upToDate` is
  // where the flow parks: unlike the index rebuild's `ready`, nothing ever
  // moves it along. A boolean cleared by the timer would be set straight back
  // to true by the next render, since the phase saying "up to date" is still
  // true — the confirmation would never go away. So each arrival at `upToDate`
  // is numbered, and the timer retires that number. Checking again numbers a
  // new one, which is what makes the second click say so as loudly as the
  // first. Adjusted during render, not in an effect
  // (.claude/rules/no-use-effect.md), the same shape as `CaptureToast`'s
  // dismissal record.
  const [checkEpisode, setCheckEpisode] = useState(0);
  const [retiredEpisode, setRetiredEpisode] = useState(0);
  const [wasUpToDate, setWasUpToDate] = useState(false);
  const isUpToDate = phase.status === "upToDate";
  if (isUpToDate !== wasUpToDate) {
    setWasUpToDate(isUpToDate);
    if (isUpToDate) setCheckEpisode((episode) => episode + 1);
  }
  const confirmed = isUpToDate && checkEpisode > retiredEpisode;
  useTimeout(
    () => setRetiredEpisode(checkEpisode),
    confirmed ? CONFIRMATION_MS : null,
    checkEpisode,
  );

  const busy =
    phase.status === "checking" ||
    phase.status === "downloading" ||
    phase.status === "installing";

  return (
    <Row
      label="Updates"
      hint="Kodabi checks for a new version at startup. Nothing downloads or installs until you say so."
      foot={
        <>
          {confirmed && <ConfirmLine>You are on the latest version.</ConfirmLine>}
          {phase.status === "error" && (
            <StatusMessage variant="error" compact>
              {phase.message}
            </StatusMessage>
          )}
        </>
      }
    >
      {phase.status === "available" || phase.status === "downloading" ? (
        <Button
          onClick={() => void download()}
          loading={phase.status === "downloading"}
          loadingLabel="Downloading…"
        >
          Download {phase.version}
        </Button>
      ) : phase.status === "readyToInstall" || phase.status === "installing" ? (
        <Button
          onClick={() => void install()}
          loading={phase.status === "installing"}
          loadingLabel="Installing…"
        >
          Restart and update
        </Button>
      ) : (
        <Button onClick={() => void check()} loading={busy} loadingLabel="Checking…">
          {phase.status === "error" && phase.step !== "check"
            ? "Try again"
            : "Check for updates"}
        </Button>
      )}
    </Row>
  );
}

/**
 * Settings — the app's CONFIG PANEL, and it announces that before a word is
 * read: cards stacked one per row at the panel's full width, each named by a
 * mono eyebrow, holding nothing but label-and-control rows.
 *
 * Every control lands on one right edge for the whole page, so the column is
 * scannable end to end. Controls are glass; read-only values stay plain text or
 * mono and take no chevron, so "you can change this" and "this is how it is"
 * are told apart by shape rather than by being greyed out.
 *
 * What is gone from the previous version is every indent, and the tab rail that
 * made three of the four concerns invisible at any one time. A dependent
 * control sits inline beside its parent instead; independent controls name
 * themselves.
 */
export function SettingsView() {
  const { settings, error, setSettings } = useSettings();
  const {
    state: { appVersion },
  } = useUpdaterStatus();

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
  // Seeded from storage during render rather than an effect — both are plain
  // synchronous reads (src/reduceMotion.ts, src/contrast.ts).
  const [reduceMotion, setReduceMotion] = useState(readReduceMotion);
  const [contrast, setContrast] = useState(readContrast);

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
  // event and started describing a state. The error lines on this screen are
  // deliberately not on a timer.
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
      setMicTestError(
        backendCopy(
          err,
          "The mic test didn't finish. Check that your microphone is connected, then run it again.",
        ),
      );
    } finally {
      setRunningMicTest(false);
    }
  };

  // `acknowledge` is the day field saying "this commit was mine", and only it
  // ever passes it. The Select raising "Saved." too meant that merely choosing
  // "Keep for a number of days" announced a save of a day count nobody had
  // touched.
  //
  // The Select needs no acknowledgement of its own, and per docs/DESIGN_SYSTEM.md
  // §3 it may not have one: a control that visibly moved is its own
  // confirmation, which is why Theme has none either. The day field is the
  // exception that whole state exists for — it persists on Enter or blur and
  // the number it holds looks identical saved or unsaved, so it is the one
  // control here whose success is not self-evident.
  const apply = async (
    nextKind: RetentionKind,
    nextDays: number,
    acknowledge = false,
  ) => {
    setSaveError(null);
    setSavingRetention(true);
    try {
      const updated = await setRetentionPolicy(buildRetentionPolicy(nextKind, nextDays));
      setSettings(updated);
      if (acknowledge) {
        setDaysSaved(true);
        setDaysSavedTick((tick) => tick + 1);
      }
    } catch (err) {
      setSaveError(
        backendCopy(
          err,
          "Couldn't save the retention policy. The previous policy still applies; try again.",
        ),
      );
      setDaysSaved(false);
    } finally {
      setSavingRetention(false);
    }
  };

  // Both flags travel together (the command takes the whole struct), so each
  // toggle sends the stored pair with its own field replaced. Its own error
  // slot, not the retention one: an error has to appear beside the control that
  // failed, or it reads as a failure of whatever it sits under. One slot for
  // both switches is right, not a shortcut: this command writes the whole
  // struct and holds one error, so the pair IS the unit that fails — and the
  // slot sits in the second of the two rows, which is where the pair ends.
  const applyOverlay = async (change: Partial<OverlaySettings>) => {
    if (!settings) return;
    setOverlayError(null);
    setSavingOverlay(true);
    try {
      const updated = await setCaptureOverlay({ ...settings.overlay, ...change });
      setSettings(updated);
    } catch (err) {
      setOverlayError(
        backendCopy(
          err,
          "Couldn't save the capture pill setting. The previous setting still applies; try again.",
        ),
      );
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
      setAppearanceError(
        backendCopy(
          err,
          "Couldn't save the theme. The previous theme still applies; try again.",
        ),
      );
    } finally {
      setSavingAppearance(false);
    }
  };

  return (
    <ViewFrame variant="panel" eyebrow="System" title="Settings">
      {error && (
        <StatusMessage variant="error">{error}</StatusMessage>
      )}

      {settings && (
        // FULL WIDTH, ONE CARD PER ROW, and no cap on the stack. A card is
        // structure, and structure fills the space it is given
        // (docs/DESIGN_SYSTEM.md §1) — the measures on this screen belong to
        // the prose inside a row (a hint stops at 46ch), not to the card
        // holding it. The row recipe puts every control flush right, so at any
        // panel width they all land on one right edge.
        <div className="mt-6 flex flex-col gap-4">
          <Card title="Privacy">
            <Row label="Recording consent">
              {/* Read-only, so it stays plain text with a check glyph — no
                  chip, no chevron, nothing that invites a click.

                  THE CHECK IS GATED ON THE VALUE. It used to render
                  unconditionally, so a profile that had never been shown the
                  consent nudge got a tick beside the words "Not shown yet" — a
                  false affirmative on the one row in the app that reports a
                  privacy state. `read` rather than `faint` for the same reason:
                  this is a fact the user may need to read, not a caption
                  (docs/DESIGN_SYSTEM.md §6). */}
              <span className="inline-flex items-center gap-1.5 text-[13px] text-ink-read">
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

            <Row
              label="Retention"
              hint="Kodabi deletes raw transcripts and recordings past this age. Your notes are never removed."
              foot={
                <>
                  {daysSaved && kind === "keep_days" && <ConfirmLine>Saved.</ConfirmLine>}
                  {saveError && (
                    <StatusMessage variant="error" compact>
                      {saveError}
                    </StatusMessage>
                  )}
                </>
              }
            >
              <Select
                hideLabel
                label="Retention"
                value={kind}
                onChange={(value) => apply(value as RetentionKind, Number(days))}
                options={RETENTION_OPTIONS}
                busy={savingRetention}
              />
              {/* The dependent control, INLINE beside the policy it depends on
                  rather than indented under it: it appears only under
                  keep_days, and appearing beside the thing that summoned it is
                  a clearer claim than a nested row was. It reads in the data
                  face because it is a number you compare, right-aligned so the
                  digits line up with themselves as they change.

                  "Days", not "Days to keep": the row it sits in is named
                  Retention and the policy is a word away, so the field only has
                  to name what it ADDS. ConsentNudge keeps the long form on
                  purpose — that dialog is a flat column of fields with no row
                  to lean on. */}
              {kind === "keep_days" && (
                <input
                  type="number"
                  min={1}
                  value={days}
                  aria-label="Days"
                  className="focus-ring w-20 rounded-button border border-edge bg-wash px-2.5 py-2 text-right font-data text-[12.5px] text-ink caret-kodama shadow-[inset_0_1px_0_var(--color-edge-lit)] transition-[border-color] duration-140 ease-out-strong outline-hidden focus:border-ink-faint"
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
                      void apply("keep_days", Number(days), true);
                    }
                  }}
                  onBlur={() => apply("keep_days", Number(days), true)}
                />
              )}
            </Row>
          </Card>

          <Card title="Appearance">
            <Row
              label="Theme"
              // Only true on `system`, so only said there. It used to render
              // under every choice, telling a user who had just picked Light
              // that the app follows their OS.
              hint={
                settings.appearance.theme === "system"
                  ? "Follows your OS light and dark setting."
                  : undefined
              }
              foot={
                appearanceError && (
                  <StatusMessage variant="error" compact>
                    {appearanceError}
                  </StatusMessage>
                )
              }
            >
              <Select
                hideLabel
                label="Theme"
                value={settings.appearance.theme}
                onChange={(value) => applyAppearanceTheme(value as Theme)}
                options={THEME_OPTIONS}
                busy={savingAppearance}
              />
            </Row>

            <Row
              label="Reduce motion"
              hint="Nothing slides, grows or spins. Fades and colour changes stay. Your OS setting already does this; here it applies to Kodabi alone."
            >
              <Switch
                label="Reduce motion"
                checked={reduceMotion}
                onChange={(next) => {
                  // Applied in the handler, not an effect: it happens because
                  // the user did something (.claude/rules/no-use-effect.md).
                  setReduceMotion(next);
                  applyReduceMotion(next);
                }}
              />
            </Row>

            <Row
              label="Increase contrast"
              hint="Faint text and hairline edges take a stronger value. Your OS setting already does this; here it applies to Kodabi alone."
            >
              <Switch
                label="Increase contrast"
                checked={contrast}
                onChange={(next) => {
                  // Applied in the handler, not an effect: it happens because
                  // the user did something (.claude/rules/no-use-effect.md).
                  setContrast(next);
                  applyContrast(next);
                }}
              />
            </Row>
          </Card>

          <Card title="Capture">
            <Row label="Global shortcut" hint="Starts and stops a capture from anywhere.">
              {/* Mono, because it is a key sequence — the same voice the
                  palette hint and every path in the app uses. */}
              <span className="font-data text-[12.5px] text-ink-read">
                {CAPTURE_SHORTCUT}
              </span>
            </Row>

            {/* The two pill switches used to be an indented group under a
                "Capture pill" heading, because their labels ("Captures you
                start") only meant something with that heading above them. They
                name themselves now, which is what let the group go: two
                independent booleans reading as two independent rows, with no
                indent claiming a dependency that was never there. */}
            <Row
              label="Pill for captures you start"
              hint="Stays on top of full screen apps, so a recording is never invisible."
            >
              <Switch
                label="Pill for captures you start"
                checked={settings.overlay.manual_captures}
                busy={savingOverlay}
                onChange={(checked) => applyOverlay({ manual_captures: checked })}
              />
            </Row>

            <Row
              label="Pill for auto detected captures"
              hint="Kodabi does not detect meetings on its own yet, so this has nothing to act on today."
              // The pair's one error slot, at the foot of the pair. See
              // applyOverlay: the command writes the whole struct and holds one
              // error, so the two switches are the unit that fails.
              foot={
                overlayError && (
                  <StatusMessage variant="error" compact>
                    {overlayError}
                  </StatusMessage>
                )
              }
            >
              <Switch
                label="Pill for auto detected captures"
                checked={settings.overlay.auto_captures}
                busy={savingOverlay}
                onChange={(checked) => applyOverlay({ auto_captures: checked })}
              />
            </Row>

            <Row
              label="Echo check"
              hint="Plays a short tone through your speakers and listens for it on your microphone, so you can see whether the two stay separate."
              foot={
                <>
                  {/* A stored fact rather than an announcement, so it is the
                      one status line on this screen with no timer: it stays
                      until another test replaces it. */}
                  {settings.mic_check && !runningMicTest && (
                    <ConfirmLine>
                      {micCheckSummary(settings.mic_check)} Checked{" "}
                      {new Date(settings.mic_check.measured_at).toLocaleString()}.
                    </ConfirmLine>
                  )}
                  {micTestError && (
                    <StatusMessage variant="error" compact>
                      {micTestError}
                    </StatusMessage>
                  )}
                </>
              }
            >
              <Button
                onClick={runMicTestClick}
                loading={runningMicTest}
                loadingLabel="Testing…"
              >
                Run test
              </Button>
            </Row>
          </Card>

          <Card title="Glossary">
            <GlossaryControl />
          </Card>

          <Card title="Models">
            <ModelsControl />
            <Row
              label="Attribution"
              hint={
                <>
                  Speech recognition uses Parakeet TDT 0.6b v2 by NVIDIA,
                  licensed under CC BY 4.0
                  (creativecommons.org/licenses/by/4.0). The model files are
                  converted to ONNX and quantized to int8 by the sherpa-onnx
                  project. Note search uses bge-small-en-v1.5 by BAAI, MIT
                  licensed.
                </>
              }
            />
          </Card>

          <Card title="Maintenance">
            <RebuildIndexControl />
          </Card>

          <Card title="About">
            <Row
              label="Version"
              hint="The build you are running right now."
            >
              {/* Mono and plain, no control: a read-only fact must never look
                  like something you can change. Same recipe as the capture
                  shortcut above. */}
              <span className="font-data text-[12.5px] text-ink-read">
                {appVersion ?? "Checking…"}
              </span>
            </Row>
            <UpdateControl />
          </Card>
        </div>
      )}
    </ViewFrame>
  );
}
