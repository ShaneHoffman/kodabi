import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CAPTURE_TOGGLE_SHORTCUT } from "../../captureControl";
import { backendCopy } from "../../errorCopy";
import {
  BUILTIN_CATEGORY_DEFAULTS,
  CATEGORY_SETTING_KEYS,
  buildRetentionPolicy,
  clampConfidence,
  clampDays,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  runMicTest,
  setAppearance,
  setCaptureOverlay,
  setCategoryPrefs,
  setIdentity,
  setLedgerTuning,
  setRetentionPolicy,
  THEME_OPTIONS,
  useSettings,
  type CategorySettings,
  type LedgerSettings,
  type MicCheckResult,
  type OverlaySettings,
  type RetentionKind,
  type Theme,
} from "../../useSettings";
import { CATEGORY_OPTIONS } from "../../useNotes";
import { useNavigation } from "../../useNavigation";
import { useShortcutStatus } from "../../useShortcutStatus";
import { applyContrast, readContrast } from "../../contrast";
import { applyReduceMotion, readReduceMotion } from "../../reduceMotion";
import { INDEX_STATE_EVENT } from "../../events";
import { formatMegabytes } from "../../models";
import { QUICK_CAPTURE_SHORTCUT } from "../../quickCapture";
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
/**
 * A number a setting commits on Enter or blur.
 *
 * Extracted because the ledger card needs three of them and Retention's
 * already-inline one made the shape: the same data face, the same right
 * alignment so digits line up with themselves as they change, and the same
 * two commit gestures. Blur alone would leave a keyboard user with no way to
 * commit deliberately (docs/DESIGN_SYSTEM.md §6).
 *
 * The value is held as a raw string by the caller so the field can be cleared
 * mid-edit rather than snapping to zero; the caller clamps on commit.
 */
function NumberField({
  label,
  value,
  min,
  max,
  width = "w-20",
  onChange,
  onCommit,
}: {
  label: string;
  value: string;
  min: number;
  max?: number;
  width?: string;
  onChange: (value: string) => void;
  onCommit: () => void;
}) {
  return (
    <input
      type="number"
      min={min}
      max={max}
      value={value}
      aria-label={label}
      className={`focus-ring ${width} rounded-button border border-edge bg-wash px-2.5 py-2 text-right font-data text-[12.5px] text-ink caret-kodama shadow-[inset_0_1px_0_var(--color-edge-lit)] transition-[border-color] duration-140 ease-out-strong outline-hidden focus:border-ink-faint`}
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          onCommit();
        }
      }}
      onBlur={onCommit}
    />
  );
}

/** A settings text field, committing on Enter and on blur.
 *
 * The text sibling of `NumberField`, and it commits the same two ways for the
 * same reason: blur alone leaves a keyboard user with no deliberate commit. */
function TextField({
  label,
  value,
  placeholder,
  width = "w-56",
  onChange,
  onCommit,
}: {
  label: string;
  value: string;
  placeholder?: string;
  width?: string;
  onChange: (value: string) => void;
  onCommit: () => void;
}) {
  return (
    <input
      type="text"
      value={value}
      placeholder={placeholder}
      aria-label={label}
      className={`focus-ring ${width} rounded-button border border-edge bg-wash px-2.5 py-2 font-ui text-[12.5px] text-ink caret-kodama shadow-[inset_0_1px_0_var(--color-edge-lit)] transition-[border-color] duration-140 ease-out-strong outline-hidden placeholder:text-ink-faint focus:border-ink-faint`}
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          onCommit();
        }
      }}
      onBlur={onCommit}
    />
  );
}

/** Which of the Identity card's two fields a commit came from. */
type IdentityField = "display_name" | "aliases";

/** The aliases field is one line of text, so the list is comma-separated going
 * in and coming out. Splitting here rather than in the backend keeps the wire
 * shape a real list: the backend trims and de-duplicates, and what it returns
 * is what gets re-seeded. */
function parseAliases(raw: string): string[] {
  return raw
    .split(",")
    .map((alias) => alias.trim())
    .filter((alias) => alias !== "");
}

/** Which of the Commitments card's three fields a commit came from. */
type LedgerField = keyof LedgerSettings;

/** Which meeting kind a per-kind write is for. */
type CategoryKey = keyof CategorySettings;

/** The three choices a kind offers, with the inherit option naming what it
 * inherits so the row reads as settled rather than blank. */
function enrollmentOptions(key: CategoryKey): { value: string; label: string }[] {
  const builtin = BUILTIN_CATEGORY_DEFAULTS[key];
  return [
    {
      value: "",
      label:
        builtin === "context_only"
          ? "Standard (only direct asks)"
          : "Standard (everything)",
    },
    { value: "tracked", label: "Track everything" },
    { value: "context_only", label: "Only direct asks" },
  ];
}

/**
 * A number field's committed value, falling back to what is stored when the
 * field is blank.
 *
 * `Number("")` is `0`, not `NaN`, so a cleared field would otherwise commit
 * zero: harmless for the day counts, which clamp back up to their minimum, and
 * the worst possible value for the auto-close confidence, where 0% closes every
 * commitment a conversation claims done without ever asking. Clearing a field
 * is how people start retyping one, so it has to mean "unchanged".
 */
function fieldNumber(raw: string, stored: number): number {
  return raw.trim() === "" ? stored : Number(raw);
}

function Row({
  label,
  hint,
  body,
  foot,
  children,
}: {
  label: string;
  hint?: ReactNode;
  /** Prose the row genuinely needs — a paragraph rather than a line. Reads one
   * register up from `hint`; see below. */
  body?: ReactNode;
  foot?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="border-t border-edge py-3 first:border-t-0">
      <div className="flex items-center justify-between gap-5">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="text-[13.5px] font-semibold text-ink">{label}</span>
          {/* The one short line a setting is allowed to explain itself with,
              capped at the measure a hint gets (docs/DESIGN_SYSTEM.md §1) and
              read in the metadata register, which §6 settles as the hint's
              home: a hint is a sentence the user MAY read, so it stays faint.
              Never a paragraph: a config panel that argues with you is a config
              panel nobody finishes reading. */}
          {hint && (
            <p className="max-w-[46ch] text-[11.5px] leading-relaxed text-ink-faint">
              {hint}
            </p>
          )}
          {/* The rare row whose explanation is genuinely prose — Attribution's
              licensing text is the only one today. Same measure and size as a
              hint, one register up: §6's carve-out is bounded by length, and a
              paragraph is prose whatever prop it arrives in. */}
          {body && (
            <p className="max-w-[46ch] text-[11.5px] leading-relaxed text-ink-dim">
              {body}
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
 * Settings' way into the vault-wide glossary, which has no home of its own:
 * every project glossary is reachable from its own project, but the vault one
 * belongs to no folder — and it is the one that matters most, since
 * transcription biases against it for every capture (a session is transcribed
 * before routing has picked a project). It is the permanent way in rather than
 * the only one: the palette's "Vault glossary" command (`useCommands`) is the
 * front door for someone who does not already know it lives here, and the model
 * nudge's ready beat points a first-run user at it once.
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
  const shortcutStatus = useShortcutStatus();

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

  // The ledger fields, raw strings for the same reason the day field is one.
  // Seeded once from storage, so an echoed settings update never overwrites an
  // edit still being typed.
  const [agingDays, setAgingDays] = useState("");
  const [staleDays, setStaleDays] = useState("");
  const [autoclose, setAutoclose] = useState("");
  const [quietDays, setQuietDays] = useState("");
  const [seededLedger, setSeededLedger] = useState(false);
  const [ledgerError, setLedgerError] = useState<string | null>(null);
  const [ledgerSaved, setLedgerSaved] = useState(false);
  const [ledgerSavedTick, setLedgerSavedTick] = useState(0);
  const [savingLedger, setSavingLedger] = useState(false);
  // Which field asked for the write, so its outcome lands under it: a `foot`
  // is the row's own status line, and an error two rows below the control that
  // raised it reads as belonging to the wrong setting.
  const [ledgerFoot, setLedgerFoot] = useState<LedgerField>("aging_after_days");
  const [displayName, setDisplayName] = useState("");
  const [aliases, setAliases] = useState("");
  const [seededIdentity, setSeededIdentity] = useState(false);
  const [identityError, setIdentityError] = useState<string | null>(null);
  const [identitySaved, setIdentitySaved] = useState(false);
  const [identitySavedTick, setIdentitySavedTick] = useState(0);
  const [savingIdentity, setSavingIdentity] = useState(false);
  // Which field asked for the write, so its outcome lands under it — the same
  // reason `ledgerFoot` exists.
  const [identityFoot, setIdentityFoot] = useState<IdentityField>("display_name");
  // The failing kind travels with its message, for the reason `ledgerFoot`
  // exists: this card is seven rows tall, so an error parked at its head reads
  // as a claim about the card rather than about the write that raised it.
  const [categoryError, setCategoryError] = useState<{
    key: CategoryKey;
    message: string;
  } | null>(null);
  // Which kind is mid-write, so only that row's Select reads as busy.
  const [savingCategory, setSavingCategory] = useState<CategoryKey | null>(null);
  if (!seededIdentity && settings) {
    setSeededIdentity(true);
    setDisplayName(settings.identity.display_name);
    setAliases(settings.identity.aliases.join(", "));
  }

  if (!seededLedger && settings) {
    setSeededLedger(true);
    setAgingDays(String(settings.ledger.aging_after_days));
    setStaleDays(String(settings.ledger.stale_after_days));
    setAutoclose(String(Math.round(settings.ledger.conversation_autoclose * 100)));
    setQuietDays(String(settings.ledger.quiet_after_days));
  }

  useTimeout(
    () => setLedgerSaved(false),
    ledgerSaved ? CONFIRMATION_MS : null,
    ledgerSavedTick,
  );

  useTimeout(
    () => setIdentitySaved(false),
    identitySaved ? CONFIRMATION_MS : null,
    identitySavedTick,
  );

  const identityFootLines = (
    <>
      {identitySaved && <ConfirmLine>Saved.</ConfirmLine>}
      {identityError && (
        <StatusMessage variant="error" compact>
          {identityError}
        </StatusMessage>
      )}
    </>
  );

  // Both fields commit the whole identity, because the backend takes it as one
  // value and the two halves are matched as a single set of spellings.
  const applyIdentity = async (field: IdentityField) => {
    // Enter then blur: the same double-commit guard the ledger fields use.
    if (!settings || savingIdentity) return;
    const merged = {
      display_name: displayName.trim(),
      aliases: parseAliases(aliases),
    };
    setIdentityFoot(field);
    const unchanged =
      merged.display_name === settings.identity.display_name &&
      merged.aliases.length === settings.identity.aliases.length &&
      merged.aliases.every((alias, index) => alias === settings.identity.aliases[index]);
    if (unchanged) {
      // Still re-seed: a trailing comma or a stray space should settle into the
      // stored spelling rather than sit there looking unsaved.
      setDisplayName(merged.display_name);
      setAliases(merged.aliases.join(", "));
      return;
    }
    setIdentityError(null);
    setSavingIdentity(true);
    try {
      const updated = await setIdentity(merged);
      setSettings(updated);
      // Re-seed from what actually persisted: the backend drops blanks and
      // duplicate spellings, so this is where a de-duplicated list shows up.
      setDisplayName(updated.identity.display_name);
      setAliases(updated.identity.aliases.join(", "));
      setIdentitySaved(true);
      setIdentitySavedTick((tick) => tick + 1);
    } catch (err) {
      setIdentityError(
        backendCopy(
          err,
          "Couldn't save your name. The previous value still applies; try again.",
        ),
      );
      setIdentitySaved(false);
    } finally {
      setSavingIdentity(false);
    }
  };

  // One pair of lines, rendered under whichever row asked for the write.
  const ledgerFootLines = (
    <>
      {ledgerSaved && <ConfirmLine>Saved.</ConfirmLine>}
      {ledgerError && (
        <StatusMessage variant="error" compact>
          {ledgerError}
        </StatusMessage>
      )}
    </>
  );

  // Every ledger field commits the whole group, because the backend takes them
  // as one struct and the two day thresholds are read against each other.
  // `field` is only for where the outcome is reported; the values all come
  // from the fields themselves.
  const applyLedger = async (field: LedgerField) => {
    // Enter commits and then blur commits again as focus leaves. The
    // unchanged-value check below catches that once a save has landed; this
    // catches the case where the second fires while the first is still in
    // flight, which would otherwise be two writes of the same value.
    if (!settings || savingLedger) return;
    const merged: LedgerSettings = {
      aging_after_days: clampDays(fieldNumber(agingDays, settings.ledger.aging_after_days)),
      stale_after_days: clampDays(fieldNumber(staleDays, settings.ledger.stale_after_days)),
      conversation_autoclose: clampConfidence(
        fieldNumber(autoclose, settings.ledger.conversation_autoclose * 100) / 100,
      ),
      quiet_after_days: clampDays(fieldNumber(quietDays, settings.ledger.quiet_after_days)),
    };
    // A cleared or clamped field shows the number the app is actually using
    // rather than what was typed, whether or not the write below happens.
    setAgingDays(String(merged.aging_after_days));
    setStaleDays(String(merged.stale_after_days));
    setAutoclose(String(Math.round(merged.conversation_autoclose * 100)));
    setQuietDays(String(merged.quiet_after_days));
    setLedgerFoot(field);
    // Nothing to say when the committed value is what was already stored: a
    // blur that changed nothing should not announce a save.
    if (
      merged.aging_after_days === settings.ledger.aging_after_days &&
      merged.stale_after_days === settings.ledger.stale_after_days &&
      merged.conversation_autoclose === settings.ledger.conversation_autoclose &&
      merged.quiet_after_days === settings.ledger.quiet_after_days
    ) {
      return;
    }
    setLedgerError(null);
    setSavingLedger(true);
    try {
      const updated = await setLedgerTuning(merged);
      setSettings(updated);
      // Re-seed from what actually persisted, so a clamped field shows the
      // number the app is using rather than the one that was typed.
      setAgingDays(String(updated.ledger.aging_after_days));
      setStaleDays(String(updated.ledger.stale_after_days));
      setAutoclose(String(Math.round(updated.ledger.conversation_autoclose * 100)));
      setQuietDays(String(updated.ledger.quiet_after_days));
      setLedgerSaved(true);
      setLedgerSavedTick((tick) => tick + 1);
    } catch (err) {
      setLedgerError(
        backendCopy(
          err,
          "Couldn't save the commitment settings. The previous values still apply; try again.",
        ),
      );
      setLedgerSaved(false);
    } finally {
      setSavingLedger(false);
    }
  };

  // One kind at a time, but the command takes the whole group, so the others
  // ride along unchanged. `""` is the inherit choice and stores `null`, which
  // is what lets a later change to the built-in defaults reach a user who never
  // touched this control.
  const applyCategory = async (key: CategoryKey, value: string) => {
    if (!settings || savingCategory) return;
    const chosen = value === "" ? null : (value as "tracked" | "context_only");
    if (settings.categories[key].enrollment_default === chosen) return;
    setCategoryError(null);
    setSavingCategory(key);
    try {
      const updated = await setCategoryPrefs({
        ...settings.categories,
        [key]: { enrollment_default: chosen },
      });
      setSettings(updated);
    } catch (err) {
      setCategoryError({
        key,
        message: backendCopy(
          err,
          "Couldn't save the meeting-kind settings. The previous values still apply; try again.",
        ),
      });
    } finally {
      setSavingCategory(null);
    }
  };

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

          {/* Ahead of Commitments, because it is what makes that card's
              Mine / Waiting on them split mean anything: without a name, every
              commitment the transcript attributes by name reads as someone
              else's. */}
          <Card title="You">
            <Row
              label="Your name"
              hint="How you are named in meetings. Commitments you take on file under Mine."
              foot={identityFoot === "display_name" && identityFootLines}
            >
              <TextField
                label="Your name"
                value={displayName}
                placeholder="Not set"
                onChange={setDisplayName}
                onCommit={() => void applyIdentity("display_name")}
              />
            </Row>

            <Row
              label="Also known as"
              hint="Other spellings meetings use for you, separated by commas. Claiming a commitment as yours adds to this list."
              foot={identityFoot === "aliases" && identityFootLines}
            >
              <TextField
                label="Other names"
                value={aliases}
                placeholder="None"
                onChange={setAliases}
                onCommit={() => void applyIdentity("aliases")}
              />
            </Row>
          </Card>

          <Card title="Commitments">
            {/* They commit as a group, but the outcome belongs to the field
                that asked for it: a `foot` is the row's own status line
                (see `Row`), so it follows the commit rather than sitting on
                whichever row happens to be last. */}
            <Row
              label="Aging"
              hint="Days without a mention before a commitment reads as aging in the Commitments view."
              foot={ledgerFoot === "aging_after_days" && ledgerFootLines}
            >
              <NumberField
                label="Days before aging"
                value={agingDays}
                min={1}
                onChange={setAgingDays}
                onCommit={() => void applyLedger("aging_after_days")}
              />
            </Row>

            <Row
              label="Stale"
              hint="Days before it reads as stale and floats to the top of its group. Nothing is ever notified; the list just reorders."
              foot={ledgerFoot === "stale_after_days" && ledgerFootLines}
            >
              <NumberField
                label="Days before stale"
                value={staleDays}
                min={1}
                onChange={setStaleDays}
                onCommit={() => void applyLedger("stale_after_days")}
              />
            </Row>

            <Row
              label="Close from conversation"
              hint="How sure a later conversation has to be that something was already done before Kodabi ticks it off for you. Below this, the claim waits for you to confirm it."
              foot={ledgerFoot === "conversation_autoclose" && ledgerFootLines}
            >
              {/* A percentage rather than the stored 0..1: "80" is a number
                  people have an intuition about, and the field is a confidence
                  the way a forecast is. Converted on both sides so the wire
                  keeps the fraction the backend validates. */}
              <NumberField
                label="Confidence percent"
                value={autoclose}
                min={0}
                max={100}
                width="w-16"
                onChange={setAutoclose}
                onCommit={() => void applyLedger("conversation_autoclose")}
              />
              <span className="font-data text-[12.5px] text-ink-faint">%</span>
            </Row>

            <Row
              label="Gone quiet"
              hint="Days without a mention before the daily digest raises something you are waiting on from someone else. Usually shorter than aging: this one is about when to go and ask."
              foot={ledgerFoot === "quiet_after_days" && ledgerFootLines}
            >
              <NumberField
                label="Days before gone quiet"
                value={quietDays}
                min={1}
                onChange={setQuietDays}
                onCommit={() => void applyLedger("quiet_after_days")}
              />
            </Row>
          </Card>

          {/* The card the taxonomy earned. Each kind now decides what reaches
              your commitments, which is the one behavior a genre was always
              going to carry. The mode is deliberately never named "observer"
              here: one of the seven kinds is literally called Observer, and a
              control whose value and whose subject share a word teaches nobody
              anything. It speaks as what it does instead. */}
          <Card title="Meeting kinds">
            <Row
              label="What each kind adds to your commitments"
              hint="Kodabi picks the kind when it writes the note; correct it from the note or the Inbox. Only direct asks keeps other people's items out, and something asked of you directly is tracked either way. A meeting's own tracking switch always wins over its kind."
            />
            {CATEGORY_OPTIONS.map((option) => {
              const key = CATEGORY_SETTING_KEYS[option.value];
              return (
                <Row
                  key={option.value}
                  label={option.label}
                  // The row's own status line, so the message sits under the
                  // Select that raised it rather than six rows above it.
                  foot={
                    categoryError?.key === key && (
                      <StatusMessage variant="error" compact>
                        {categoryError.message}
                      </StatusMessage>
                    )
                  }
                >
                  <Select
                    hideLabel
                    label={`${option.label} commitments`}
                    value={settings.categories[key].enrollment_default ?? ""}
                    onChange={(value) => void applyCategory(key, value)}
                    options={enrollmentOptions(key)}
                    busy={savingCategory === key}
                  />
                </Row>
              );
            })}
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
            <Row
              label="Global shortcut"
              hint="Starts and stops a capture from anywhere."
              // Registration is best-effort at startup, so this row is the one
              // place a refused chord can stop being a silent lie: the key does
              // nothing, and without this the screen a user would check to find
              // that out says it works. Withheld while the status is unknown
              // (the read has not landed, or failed) — `useShortcutStatus`
              // explains why no evidence must not read as bad news.
              //
              // The fallback is the tray *menu*, and this row is the only place
              // that names it — the status pill used to carry the same fallback
              // and no longer says anything on an idle pill. The tray *menu*,
              // not the icon: the icon's left click shows the main window and
              // its menu is right-click only (`show_menu_on_left_click(false)`
              // in `capture_control.rs`), so naming the icon would replace one
              // dead instruction with another.
              foot={
                shortcutStatus !== null &&
                !shortcutStatus.captureToggle && (
                  <StatusMessage variant="error" compact>
                    Unavailable: another app is using this shortcut. Use the tray menu to
                    start a capture.
                  </StatusMessage>
                )
              }
            >
              {/* Mono, because it is a key sequence — the same voice the
                  palette hint and every path in the app uses.

                  Rendered rather than editable: the backend registers the chord
                  at startup and offers no rebinding command yet, and a field
                  that silently fails to save is worse than a value that plainly
                  is what it is.

                  Shown unchanged even when the bind failed: it names *which*
                  chord is spoken for, which is what tells the user where to
                  look. The foot carries the state — fading or striking the
                  chord would put that claim in the metadata register. */}
              <span className="font-data text-[12.5px] text-ink-read">
                {CAPTURE_TOGGLE_SHORTCUT}
              </span>
            </Row>

            {/* The second of the two global chords, and the reason this card is
                the app's shortcut reference rather than a page about recording.
                Quick capture's chord is learned once and then only ever
                forgotten, and until this row it was written down nowhere a user
                would think to look: the palette prints it as a hint on a row you
                have to already be in the palette to see, and the tray item
                carries no accelerator text at all.

                Same recipe as the row above, deliberately — mono for a key
                sequence, rendered rather than editable because the backend
                registers this one at startup too. */}
            <Row label="Quick capture" hint="Opens the quick capture box from anywhere.">
              <span className="font-data text-[12.5px] text-ink-read">
                {QUICK_CAPTURE_SHORTCUT}
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
              // A paragraph, not a hint: five lines of licensing prose is the
              // case DESIGN_SYSTEM §6's carve-out excludes, so it reads at
              // ink-dim through `body`.
              body={
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
