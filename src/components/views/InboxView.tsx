import { useMemo, useState } from "react";
import {
  pipelineStage,
  savedDestination,
  savedPathMatchesNote,
  useCapturePipeline,
  type PipelineStage,
} from "../../useCapturePipeline";
import { useNavigation } from "../../useNavigation";
import { matchScore, noteMeta } from "../../noteMeta";
import {
  fileNoteToProject,
  INBOX_PROJECT,
  listNotes,
  useProjectNotes,
  type NoteSummary,
} from "../../useNotes";
import { useProjects } from "../../useProjects";
import { formatElapsed, useElapsed } from "../../useElapsed";
import { useTimeout } from "../../useTimeout";
import { Select, type SelectOption } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./InboxView.css";

/** Mirrors `--dur-settle`: how long the placeholder's travel-left-and-vanish
 * plays once distill routes it to a project, before it hands off to the toast. */
const VANISH_MS = 200;
/** How long "Filed to <project>" stays up before it fades — the same dwell
 * `CaptureToast` used to give a success notice. */
const TOAST_DWELL_MS = 3500;
/** Mirrors `--dur-settle`: the toast's own fade-out, once the dwell ends. */
const TOAST_FADE_MS = 200;
/** Covers the two ways a stage could otherwise wait forever: a dev/mock
 * build never follows a saved transcript with a distill, and an Inbox-routed
 * save's vault refetch can fail or never land. */
const GRACE_MS = 10_000;

/**
 * The Inbox: notes the router couldn't place with confidence, each with a
 * one-click file-to-project (the correction loop — FOUNDING_DOC §3.5). The
 * list reads straight from the `Inbox/` folder; filing moves the file,
 * re-scores it, and logs a routing example, then the row settles out of view.
 *
 * It is the app's one WORKING QUEUE, and the layout says so before a word is
 * read: pinned hard left, the densest gutter in the app, a compact one-line
 * masthead instead of a title, and the only rows that lift under the pointer.
 * Nothing here is for browsing. Everything here is for clearing.
 */
export function InboxView() {
  const { notes, loading, error } = useProjectNotes(INBOX_PROJECT);
  const { entries } = useProjects();
  // Counted in this session rather than read from disk: nothing persists a
  // daily tally, and inventing one that resets at midnight would be claiming
  // more than the app knows. This says exactly what it means — how many you
  // have cleared since opening the app.
  const [filedThisSession, setFiledThisSession] = useState(0);

  // Real projects only — the Inbox itself is never a filing target. Memoized
  // (entries is stable from useProjects) so the same array reference reaches
  // every row's picker and doesn't force a re-render of every row.
  const options = useMemo<SelectOption[]>(
    () =>
      entries.flatMap((entry) =>
        entry.kind === "project"
          ? // The menu lists PATHS, not display names: filing is choosing a
            // location, and a nested project's parentage is the whole reason
            // you'd pick it over its sibling.
            [{ value: entry.project.slug, label: entry.project.slug }]
          : [],
      ),
    [entries],
  );
  const projectSlugs = options.map((option) => option.value);

  // The one pipeline subscription, held above the routed view (AppShell), so
  // the placeholder below reflects the current stage even when the Inbox
  // mounts mid-pipeline.
  const stage = pipelineStage(useCapturePipeline());
  const { placeholder, toast, pauseToast, resumeToast } = usePipelinePresence(
    stage,
    notes,
    projectSlugs,
  );

  // The note an Inbox-routed distill just produced, if it has landed in this
  // list yet — purely derived, so it needs no state of its own and cannot go
  // stale: it tracks whatever `stage` is pointing at, for as long as it keeps
  // pointing at it. It gives the arriving row a one-shot fill-in animation
  // (`InboxRow`'s `fresh` prop) instead of the row simply appearing.
  const freshNotePath =
    stage?.kind === "filed" && savedDestination(stage.savedPath, projectSlugs).kind === "inbox"
      ? (notes.find((note) => savedPathMatchesNote(stage.savedPath, note.path))?.path ?? null)
      : null;

  const remaining = notes.length;
  const handled = filedThisSession + remaining;
  const cleared = handled > 0 ? (filedThisSession / handled) * 100 : 0;

  return (
    <ViewFrame
      variant="queue"
      eyebrow="Unfiled"
      title="Inbox"
      // The work, stated before the list is read. Omitted at zero: the empty
      // state below says it better, and saying it twice says it worse.
      summary={remaining > 0 ? `${remaining} to file` : undefined}
    >
      {error ? (
        <StatusMessage variant="error">Couldn&apos;t load the inbox: {error}</StatusMessage>
      ) : (
        <>
          {remaining > 0 && (
            <Progress filed={filedThisSession} remaining={remaining} percent={cleared} />
          )}
          {remaining === 0 && !placeholder ? (
            !loading && (
              <StatusMessage variant="empty">
                Nothing waiting. Notes the router can&apos;t place land here.
              </StatusMessage>
            )
          ) : (
            <ul className="inbox__list">
              {placeholder && <PipelinePlaceholder presence={placeholder} />}
              {notes.map((note) => (
                // Keyed by path, not id: two files can carry the same id (an
                // external copy), and duplicate keys would mis-reconcile rows.
                <InboxRow
                  key={note.path}
                  note={note}
                  options={options}
                  onFiled={() => setFiledThisSession((count) => count + 1)}
                  fresh={note.path === freshNotePath}
                />
              ))}
            </ul>
          )}
        </>
      )}
      {toast && <FiledToast toast={toast} onPause={pauseToast} onResume={resumeToast} />}
    </ViewFrame>
  );
}

/**
 * The placeholder's own view of the pipeline stage. Unlike `PipelineStage`,
 * it already knows where a `filed` outcome landed, and it treats "filed to
 * the Inbox, but the note hasn't shown up in this list yet" and "filed to
 * the Inbox, and it just did" as two different things — the second one is
 * `null`, which is how the placeholder hands off to the real row without
 * playing an exit of its own. "Filed to a project" carries the outcome's
 * `savedPath` too, so the toast it hands off to can resolve the exact note.
 */
type PlaceholderStage =
  | { id: string; kind: "transcribing" }
  | { id: string; kind: "distilling"; awaitingDistill: boolean }
  | { id: string; kind: "awaitingInboxHandoff" }
  | { id: string; kind: "filedToProject"; slug: string; savedPath: string };

function resolvePlaceholderStage(
  stage: PipelineStage | null,
  notes: NoteSummary[],
  projectSlugs: string[],
): PlaceholderStage | null {
  if (stage === null) return null;
  if (stage.kind === "transcribing") return { id: stage.id, kind: "transcribing" };
  if (stage.kind === "distilling") {
    return { id: stage.id, kind: "distilling", awaitingDistill: stage.awaitingDistill };
  }

  const destination = savedDestination(stage.savedPath, projectSlugs);
  if (destination.kind === "project") {
    return {
      id: stage.id,
      kind: "filedToProject",
      slug: destination.slug,
      savedPath: stage.savedPath,
    };
  }
  const alreadyListed = notes.some((note) => savedPathMatchesNote(stage.savedPath, note.path));
  return alreadyListed ? null : { id: stage.id, kind: "awaitingInboxHandoff" };
}

/** What the placeholder row shows right now: which status line is lit,
 * whether it is mid-vanish (routed to a project — traveling left and out),
 * and how long this run has been going — the real transcribe phase spans a
 * model load, the ASR pass, and a headless Claude cleanup call, so "still
 * transcribing" can hold for several real seconds with nothing else to show
 * for it. The ticking clock is the honest substitute: proof the run is
 * still alive without claiming to know which of those it's in right now. */
type PlaceholderPresence = {
  phase: "transcribing" | "distilling";
  vanishing: boolean;
  elapsedSeconds: number;
};

/** The one success announcement left after `CaptureToast` went
 * failures-only: a note filed to a different project. Inbox-owned rather
 * than shared with `CaptureToast`, so that component's failures-only
 * contract stays exactly what it says. */
type FiledToastPresence = { slug: string; savedPath: string; fading: boolean };

/**
 * What the placeholder row and the filed-toast show, and for how long: a
 * pure derivation from `resolvePlaceholderStage`, plus timers that only ever
 * move a stage toward its resolution or give up on one that never arrives.
 * All keyed on the stage's own id, so a fresh stage (a retry, a second
 * capture) never inherits a clock that belonged to the one before it.
 */
function usePipelinePresence(
  stage: PipelineStage | null,
  notes: NoteSummary[],
  projectSlugs: string[],
): {
  placeholder: PlaceholderPresence | null;
  toast: FiledToastPresence | null;
  pauseToast: () => void;
  resumeToast: () => void;
} {
  const resolved = resolvePlaceholderStage(stage, notes, projectSlugs);

  const [waivedId, setWaivedId] = useState<string | null>(null);
  const [toast, setToast] = useState<
    { id: string; slug: string; savedPath: string; fading: boolean } | null
  >(null);
  const [toastPaused, setToastPaused] = useState(false);
  const [runStartedAt, setRunStartedAt] = useState<number | null>(null);

  // The pipeline going quiet — a fresh capture starting — is the run
  // boundary: a waiver or an open toast that belonged to one capture must
  // not survive into the next one's. Adjusted during render rather than in
  // an effect (.claude/rules/no-use-effect.md), mirroring `CaptureToast`'s
  // `dismissedId` reset. Keyed on `stage`, not `resolved`: `resolved` also
  // goes null the instant an Inbox-routed note appears in the list, and that
  // silent handoff must not clear an unrelated, still-open toast.
  if (stage === null) {
    if (waivedId !== null) setWaivedId(null);
    if (toast !== null) setToast(null);
  }

  // The run clock: stamped once, the moment the placeholder first has
  // anything to show, and cleared once there is nothing left to time —
  // including the silent handoff, which is why this checks `resolved`
  // rather than `stage` (unlike the reset above). It does not restart at
  // the transcribing→distilling crossfade: that would read as the wait
  // going backwards just as the pipeline actually moved forward.
  if (resolved !== null && runStartedAt === null) setRunStartedAt(Date.now());
  if (resolved === null && runStartedAt !== null) setRunStartedAt(null);
  const elapsedSeconds = useElapsed(runStartedAt);

  const visible = resolved !== null && resolved.id !== waivedId;
  const vanishing = visible && resolved?.kind === "filedToProject";

  // Covers a dev/mock build (distill never follows a saved transcript) and
  // an Inbox-routed save whose vault refetch fails or never lands. Without
  // this the placeholder would sit on "Distilling the meeting" forever.
  const awaitingResolution =
    visible &&
    resolved !== null &&
    ((resolved.kind === "distilling" && resolved.awaitingDistill) ||
      resolved.kind === "awaitingInboxHandoff");
  useTimeout(
    () => resolved && setWaivedId(resolved.id),
    awaitingResolution ? GRACE_MS : null,
    resolved?.id ?? null,
  );

  // The vanish plays for VANISH_MS; when it finishes the placeholder is gone
  // for good and the toast it hands off to takes over the announcement.
  useTimeout(
    () => {
      if (!resolved || resolved.kind !== "filedToProject") return;
      setWaivedId(resolved.id);
      setToastPaused(false);
      setToast({
        id: resolved.id,
        slug: resolved.slug,
        savedPath: resolved.savedPath,
        fading: false,
      });
    },
    vanishing ? VANISH_MS : null,
    resolved?.id ?? null,
  );

  // The toast's own lifecycle: dwell (paused while the user is reading or
  // about to click it), then fade, then gone.
  useTimeout(
    () => setToast((current) => (current ? { ...current, fading: true } : current)),
    toast && !toast.fading && !toastPaused ? TOAST_DWELL_MS : null,
    toast?.id ?? null,
  );
  useTimeout(
    () => setToast(null),
    toast?.fading ? TOAST_FADE_MS : null,
    toast?.id ?? null,
  );

  const placeholder: PlaceholderPresence | null =
    visible && resolved
      ? {
          phase: resolved.kind === "transcribing" ? "transcribing" : "distilling",
          vanishing,
          elapsedSeconds,
        }
      : null;

  return {
    placeholder,
    toast: toast ? { slug: toast.slug, savedPath: toast.savedPath, fading: toast.fading } : null,
    pauseToast: () => setToastPaused(true),
    resumeToast: () => setToastPaused(false),
  };
}

/**
 * The placeholder row: it wears a real row's silhouette from the moment it
 * arrives — the same title/meta slots, entering with the one deliberate
 * motion the app spends on distill-and-route (FOUNDING_DOC §4) — so
 * resolving into a real note is a fill-in, never a swap. It never lifts
 * under the pointer (it isn't actionable yet) and never turns green: that
 * colour means audio is being recorded, and this runs entirely after a
 * capture has already stopped (docs/UI_CONVENTIONS.md).
 *
 * The status and meta lines are two texts stacked in place, crossfading as
 * `phase` advances, rather than one line rewriting its content — the same
 * node is what makes the `role="status"` region below reliably announce the
 * change (docs/DESIGN_SYSTEM.md §6).
 */
function PipelinePlaceholder({ presence }: { presence: PlaceholderPresence }) {
  return (
    <li
      className={`inbox__slot${presence.vanishing ? " inbox__slot--leaving" : ""}`}
      data-testid="pipeline-placeholder"
    >
      <div>
        <div
          className={`inbox__row inbox__row--placeholder${
            presence.vanishing ? " inbox__row--vanishing" : ""
          }`}
        >
          <div role="status">
            <p className="inbox__pipeline-title text-row font-semibold tracking-row text-text-soft">
              <span className="inbox__pipeline-dot" aria-hidden="true" />
              <span className="inbox__pipeline-stack">
                <span className={presence.phase === "transcribing" ? "is-visible" : ""}>
                  Transcribing the capture
                </span>
                <span className={presence.phase === "distilling" ? "is-visible" : ""}>
                  Distilling the meeting
                </span>
              </span>
            </p>
            <p className="mt-2xs font-mono text-cap text-text-faint">
              <span className="inbox__pipeline-stack">
                <span className={presence.phase === "transcribing" ? "is-visible" : ""}>
                  just stopped · queued
                </span>
                <span className={presence.phase === "distilling" ? "is-visible" : ""}>
                  reading transcript · routing
                </span>
              </span>
              {/* A running clock, not a phase label: the real transcribe
                  phase alone can hold "Transcribing the capture" for several
                  seconds (model load, ASR, a headless Claude cleanup call),
                  and a ticking number is the honest way to show the run is
                  still alive without claiming to know which of those it's
                  in right now. */}
              <span className="ui-tnum"> · {formatElapsed(presence.elapsedSeconds)}</span>
            </p>
          </div>
          <div />
        </div>
      </div>
    </li>
  );
}

/**
 * The distill pipeline's one success announcement: a note filed to a
 * different project, reported as a whole-surface affordance rather than a
 * plain confirmation — clicking it opens the note it just described. It is
 * Inbox-owned rather than folded into `CaptureToast`, which stays
 * failures-only on purpose (docs/DESIGN_SYSTEM.md §6): this is the payoff
 * the placeholder's vanish hands off to, not a failure anyone needs to see
 * from another screen.
 *
 * The dwell pauses while hovered or focused, so the toast cannot vanish out
 * from under a pointer that is already headed for it.
 */
function FiledToast({
  toast,
  onPause,
  onResume,
}: {
  toast: FiledToastPresence;
  onPause: () => void;
  onResume: () => void;
}) {
  const { navigate } = useNavigation();

  // The distill `saved` event carries the note's path, not its id, so
  // opening it is a small lookup rather than a direct jump. A note that
  // moved again before the click (re-filed, renamed) falls back to the
  // project it reported, which is still the true answer.
  const open = () => {
    listNotes(toast.slug)
      .then((projectNotes) => {
        const note = projectNotes.find((candidate) =>
          savedPathMatchesNote(toast.savedPath, candidate.path),
        );
        navigate(
          note
            ? { kind: "noteEditor", noteId: note.id, project: toast.slug }
            : { kind: "project", slug: toast.slug },
        );
      })
      .catch(() => navigate({ kind: "project", slug: toast.slug }));
  };

  return (
    <button
      type="button"
      className={`inbox__toast ui-focus-ring${toast.fading ? " inbox__toast--fading" : ""}`}
      onClick={open}
      onMouseEnter={onPause}
      onMouseLeave={onResume}
      onFocus={onPause}
      onBlur={onResume}
      aria-label={`Open the note filed to "${toast.slug}"`}
    >
      <span role="status">
        <span className="font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint">
          Filed to
        </span>
        <span className="mt-3xs text-label font-semibold text-text">{toast.slug}</span>
      </span>
      <span className="inbox__toast__arrow text-text-soft" aria-hidden="true">
        →
      </span>
    </button>
  );
}

/**
 * How much of the queue you have cleared, as a rule and a sentence.
 *
 * It sits between the masthead and the list because that is where a progress
 * reading is useful — before you start, not after you scroll. At zero filed it
 * still renders: an empty track that says "4 to go" is the honest starting
 * state, and a bar that only appears once you are winning is a bar that never
 * helps you begin.
 */
function Progress({
  filed,
  remaining,
  percent,
}: {
  filed: number;
  remaining: number;
  percent: number;
}) {
  return (
    <div className="inbox__progress mt-sm">
      {/* aria-hidden, not role="progressbar". The bar is a redrawing of the
          caption directly beneath it, which already says both numbers in
          words; announcing it as a progressbar would report the same fact
          twice, once as a percentage nobody asked for. One region per
          concern (docs/DESIGN_SYSTEM.md §6). */}
      <div className="inbox__track" aria-hidden="true">
        <div className="inbox__fill" style={{ width: `${percent}%` }} />
      </div>
      {/* text-micro, not text-eyebrow. §1 reserves the eyebrow step for a
          section label and requires it to be mono + uppercase + a tracking
          step; this is neither, and §1 names text-micro for exactly this
          role ("a progress caption"). Tabular figures because both numbers
          change as the queue is cleared. */}
      <p className="ui-tnum mt-2xs font-mono text-micro text-text-faint">
        {filed} filed this session · {remaining} to go
      </p>
    </div>
  );
}

/**
 * One unfiled note. The title opens it; the picker in the right gutter files
 * it. On success the row plays its collapse/fade exit; the file command
 * broadcasts `vault:changed` itself, which refetches the list and the sidebar
 * count together and drops the row (the file watcher is a fallback for
 * external edits, not the only trigger, so the row leaves even if the watcher
 * never started). A failed file keeps the row and surfaces the message.
 *
 * `fresh` marks the one row that just finished being the pipeline
 * placeholder: it plays a one-shot fill-in on mount instead of simply being
 * present, so the placeholder becoming this row reads as continuous rather
 * than as a swap.
 */
function InboxRow({
  note,
  options,
  onFiled,
  fresh,
}: {
  note: NoteSummary;
  options: SelectOption[];
  onFiled: () => void;
  fresh: boolean;
}) {
  const { navigate } = useNavigation();
  const [pending, setPending] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const route = (slug: string) => {
    setPending(true);
    setError(null);
    fileNoteToProject({ id: note.id, project: slug })
      .then(() => {
        onFiled();
        setLeaving(true);
      })
      .catch((err: unknown) => {
        setPending(false);
        setError(String(err));
      });
  };

  return (
    <li className={`inbox__slot${leaving ? " inbox__slot--leaving" : ""}`}>
      <div>
        <div className={`inbox__row${fresh ? " inbox__row--fresh" : ""}`}>
          <div>
            <button
              type="button"
              className="inbox__open ui-focus-ring text-row font-semibold tracking-row text-text"
              onClick={() =>
                navigate({
                  kind: "noteEditor",
                  noteId: note.id,
                  project: INBOX_PROJECT,
                })
              }
            >
              {note.title}
            </button>
            <p className="mt-2xs font-mono text-cap text-text-faint">
              {noteMeta(note, matchScore(note.confidence))}
            </p>
            {note.snippet && (
              <p className="inbox__snippet mt-2xs font-serif text-snippet leading-snippet text-text-soft">
                {note.snippet}
              </p>
            )}
          </div>
          <div>
            {options.length === 0 ? (
              // No picker at all when there is nothing to file into: a control
              // whose only outcome is a dead end should not be offered.
              // variant="empty", not "status": the variant fixes the ARIA role,
              // and a static sentence repeated once per row must not be N live
              // regions (docs/DESIGN_SYSTEM.md §3).
              <StatusMessage variant="empty" compact>
                Create a project to file notes.
              </StatusMessage>
            ) : (
              <Select
                hideLabel
                variant="token"
                label={`File "${note.title}" to project`}
                value={null}
                placeholder={pending ? "Filing" : "File"}
                options={options}
                busy={pending}
                onChange={route}
              />
            )}
          </div>
        </div>
        {error && (
          <StatusMessage variant="error" compact>
            Couldn&apos;t file this note: {error}
          </StatusMessage>
        )}
      </div>
    </li>
  );
}
