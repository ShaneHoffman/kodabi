import { clsx } from "clsx";
import { useMemo, useState } from "react";
import {
  pipelineStage,
  savedDestination,
  savedPathMatchesNote,
  useCapturePipeline,
  type CapturePipeline,
  type PipelineStage,
} from "../../useCapturePipeline";
import {
  LIST_ROW_ENTER,
  LIST_ROW_EXIT,
  LIST_ROW_LEAVING,
  LIST_SLOT,
  LIST_SLOT_INNER,
  LIST_SLOT_INNER_LEAVING,
  LIST_SLOT_LEAVING,
} from "../../listMotion";
import { useNavigation } from "../../useNavigation";
import { matchScore, noteKind } from "../../noteMeta";
import {
  fileNoteToProject,
  INBOX_PROJECT,
  listNotes,
  useProjectNotes,
  type NoteSummary,
} from "../../useNotes";
import { folderHue, useProjects, type FolderHue, type Project } from "../../useProjects";
import { isSessionSource } from "../../useSessions";
import { formatElapsed, useElapsed } from "../../useElapsed";
import { useTimeout } from "../../useTimeout";
import { SpiritMark } from "../capture/SpiritMark";
import { CreateProjectDialog } from "../dialogs/CreateProjectDialog";
import { DeleteNoteDialog } from "../dialogs/DeleteNoteDialog";
import { Button } from "../ui/Button";
import { Menu } from "../ui/Menu";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

/** Mirrors `--dur-settle`: how long the placeholder's travel-left-and-vanish
 * plays once distill routes it to a project, before it hands off to the toast. */
const VANISH_MS = 200;
/** How long "Filed to <project>" stays up before it fades — the same dwell
 * `CaptureToast` used to give a success notice. */
const TOAST_DWELL_MS = 3500;
/** Mirrors `--dur-exit`: the toast's own fade-out, once the dwell ends. It is
 * shorter than the entrance it undoes, which is the rule for every exit in the
 * app (docs/DESIGN_SYSTEM.md §4). */
const TOAST_FADE_MS = 130;
/** Covers `--dur-plane` with a little room: how long after an Inbox-routed
 * note lands before its outcome counts as fully presented — the fill-in has
 * played and there is nothing left for a remount to replay. */
const FILL_IN_MS = 200;
/** Covers the three ways a stage could otherwise wait forever: a stop the
 * backend never acknowledges (a mis-tap under its minimum session duration
 * emits nothing at all), a dev/mock build never following a saved transcript
 * with a distill, and an Inbox-routed save whose vault refetch fails or
 * never lands. */
const GRACE_MS = 10_000;

/* The folder hues, written out rather than interpolated: Tailwind cannot see a
   constructed class name and would emit none of these
   (docs/UI_CONVENTIONS.md). The card edge, the guess words and the menu dot all
   read from the same three tables, so a project is one colour wherever it
   appears. */
const GUESS_EDGE: Record<FolderHue, string> = {
  coral: "border-l-coral",
  cobalt: "border-l-cobalt",
  teal: "border-l-teal",
  plum: "border-l-plum",
};
const GUESS_TEXT: Record<FolderHue, string> = {
  coral: "text-coral",
  cobalt: "text-cobalt",
  teal: "text-teal",
  plum: "text-plum",
};
const HUE_DOT: Record<FolderHue, string> = {
  coral: "bg-coral",
  cobalt: "bg-cobalt",
  teal: "bg-teal",
  plum: "bg-plum",
};

/**
 * How strong the router's suggestion has to be before the card wears it.
 *
 * The backend guess has no threshold of its own — it answers "who won" even
 * when the winner won by nothing (a dead tie scores 0.0). A colour and a
 * destination on the card are a claim, and a claim built on a coin flip is
 * worse than an honest blank, so the display floor lives here rather than in
 * the router: this is a presentation decision about how loudly to offer a
 * hint, not a decision about where notes go.
 */
const GUESS_FLOOR = 0.2;

/** The number of segments in the progress meter. Five is a reading, not a
 * measurement: the caption beside it carries the actual counts. */
const METER_SEGMENTS = 5;

/** The suggested destination a card offers, or `null` when it has none worth
 * offering. One derivation, read by the edge, the meta line and the menu. */
function suggestion(note: NoteSummary): { slug: string; hue: FolderHue } | null {
  if (!note.guess || note.guess.confidence < GUESS_FLOOR) return null;
  return { slug: note.guess.project, hue: folderHue(note.guess.project) };
}

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
  // every row's menu and doesn't force a re-render of every row.
  const projects = useMemo<Project[]>(
    () => entries.flatMap((entry) => (entry.kind === "project" ? [entry.project] : [])),
    [entries],
  );
  const projectSlugs = projects.map((project) => project.slug);

  // The one pipeline subscription, held above the routed view (AppShell), so
  // the placeholder below reflects the current stage even when the Inbox
  // mounts mid-pipeline.
  const pipeline = useCapturePipeline();
  const stage = pipelineStage(pipeline);
  const { placeholder, toast, pauseToast, resumeToast } = usePipelinePresence(
    pipeline,
    stage,
    notes,
    projectSlugs,
  );

  // The note an Inbox-routed distill just produced, if it has landed in this
  // list yet — purely derived, so it needs no state of its own and cannot go
  // stale: it tracks whatever `stage` is pointing at, for as long as it keeps
  // pointing at it. It gives the arriving row a one-shot fill-in animation
  // (`InboxRow`'s `fresh` prop) instead of the row simply appearing. Once the
  // fill-in has settled the outcome is marked handled (see
  // `usePipelinePresence`), which is what keeps a remount from replaying it.
  const freshNotePath =
    stage?.kind === "filed" &&
    stage.id !== pipeline.handledFiledId &&
    savedDestination(stage.savedPath, projectSlugs).kind === "inbox"
      ? (notes.find((note) => savedPathMatchesNote(stage.savedPath, note.path))?.path ?? null)
      : null;

  const remaining = notes.length;
  const handled = filedThisSession + remaining;
  const cleared = handled > 0 ? filedThisSession / handled : 0;

  return (
    <ViewFrame
      variant="queue"
      eyebrow="Unfiled"
      title="Inbox"
      // The whole state of the work in one line: what is left, and what you
      // have already done about it. Omitted at zero — the empty state below
      // says it better, and saying it twice says it worse.
      summary={
        remaining > 0
          ? `${remaining} unfiled · ${filedThisSession} filed this session`
          : undefined
      }
    >
      {error ? (
        <StatusMessage variant="error">Couldn&apos;t load the inbox: {error}</StatusMessage>
      ) : (
        <>
          {remaining > 0 && <Progress cleared={cleared} />}
          {remaining === 0 && !placeholder ? (
            !loading && <EmptyInbox />
          ) : (
            <ul data-testid="inbox-list" className="flex flex-col gap-3.5">
              {placeholder && <PipelinePlaceholder presence={placeholder} />}
              {notes.map((note) => (
                // Keyed by path, not id: two files can carry the same id (an
                // external copy), and duplicate keys would mis-reconcile rows.
                <InboxRow
                  key={note.path}
                  note={note}
                  projects={projects}
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
 * Nothing to file — the one screen in the app that is good news.
 *
 * First-run copy, not an apology (docs/DESIGN_SYSTEM.md §6): an idle kodama, a
 * statement, and one line saying what would be here if there were anything.
 * The mark is still — `idle` is the mode with no aura animation at all, which
 * is the point. An empty queue is not the system working, and a creature
 * breathing over "nothing waiting" would suggest it was.
 */
function EmptyInbox() {
  return (
    <div className="flex flex-col items-center gap-1.5 pt-24 pb-16 text-center">
      <SpiritMark mode="idle" size="26px" className="mb-[18px]" />
      <p className="text-[15px] font-semibold text-ink">Nothing waiting.</p>
      <p className="max-w-[44ch] text-[12.5px] leading-[1.55] text-ink-faint">
        Notes the router can&apos;t place with confidence land here. Right now everything is
        filed where it belongs.
      </p>
    </div>
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
  | { id: string; kind: "transcribing"; awaitingTranscribe: boolean }
  | { id: string; kind: "distilling"; awaitingDistill: boolean }
  | { id: string; kind: "awaitingInboxHandoff" }
  | { id: string; kind: "filedToProject"; slug: string; savedPath: string };

function resolvePlaceholderStage(
  stage: PipelineStage | null,
  notes: NoteSummary[],
  projectSlugs: string[],
  handledFiledId: string | null,
): PlaceholderStage | null {
  if (stage === null) return null;
  if (stage.kind === "transcribing") {
    return { id: stage.id, kind: "transcribing", awaitingTranscribe: stage.awaitingTranscribe };
  }
  if (stage.kind === "distilling") {
    return { id: stage.id, kind: "distilling", awaitingDistill: stage.awaitingDistill };
  }

  // An outcome the Inbox has already fully presented (vanish-and-toast
  // played, or the routed row's fill-in settled) is over, however long the
  // distill hook keeps reporting it. Checked before the listed-note probe
  // below: a landed note that later leaves the list (the user filed it
  // somewhere else) must not resurrect this as a phantom "distilling" row.
  if (stage.id === handledFiledId) return null;

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
  pipeline: CapturePipeline,
  stage: PipelineStage | null,
  notes: NoteSummary[],
  projectSlugs: string[],
): {
  placeholder: PlaceholderPresence | null;
  toast: FiledToastPresence | null;
  pauseToast: () => void;
  resumeToast: () => void;
} {
  const resolved = resolvePlaceholderStage(stage, notes, projectSlugs, pipeline.handledFiledId);

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

  // Covers a stop the backend never acknowledges (a mis-tap under its
  // minimum session duration emits nothing at all), a dev/mock build
  // (distill never follows a saved transcript), and an Inbox-routed save
  // whose vault refetch fails or never lands. Without this the placeholder
  // would sit on "Transcribing the capture" or "Distilling the meeting"
  // forever.
  const awaitingResolution =
    visible &&
    resolved !== null &&
    ((resolved.kind === "transcribing" && resolved.awaitingTranscribe) ||
      (resolved.kind === "distilling" && resolved.awaitingDistill) ||
      resolved.kind === "awaitingInboxHandoff");
  useTimeout(
    () => {
      if (!resolved) return;
      setWaivedId(resolved.id);
      // A given-up synthetic stop is retired at the shell too: `stopPending`
      // outlives this view's `waivedId`, and without clearing it every later
      // mount would replay the phantom placeholder for another grace period.
      if (resolved.id === "stopped") pipeline.markStopHandled();
    },
    awaitingResolution ? GRACE_MS : null,
    resolved?.id ?? null,
  );

  // The vanish plays for VANISH_MS; when it finishes the placeholder is gone
  // for good — marked handled at the shell, so a remount cannot replay it —
  // and the toast it hands off to takes over the announcement.
  useTimeout(
    () => {
      if (!resolved || resolved.kind !== "filedToProject") return;
      pipeline.markFiledHandled(resolved.id);
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

  // The silent handoff's equivalent: an Inbox-routed note has landed in the
  // list, its row's fill-in has had time to play, and the outcome is over.
  // Without this, the stage would stay live until the next capture, and a
  // landed note that later left the list (filed elsewhere by the user) would
  // resurrect the placeholder as a phantom "distilling" row.
  const inboxLanded =
    stage?.kind === "filed" &&
    stage.id !== pipeline.handledFiledId &&
    savedDestination(stage.savedPath, projectSlugs).kind === "inbox" &&
    notes.some((note) => savedPathMatchesNote(stage.savedPath, note.path));
  useTimeout(
    () => {
      if (stage?.kind === "filed") pipeline.markFiledHandled(stage.id);
    },
    inboxLanded ? FILL_IN_MS : null,
    stage?.id ?? null,
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
 * `phase` advances, rather than one line rewriting its content — so the box
 * never reflows. Both texts existing at once (and the once-a-second clock)
 * is a visual trick, though, so the whole visual layer is `aria-hidden` and
 * the `role="status"` region announces through one sr-only line that
 * genuinely rewrites as the stage advances (docs/DESIGN_SYSTEM.md §6) — a
 * class flip alone would announce nothing, and a live clock would announce
 * every second.
 */
function PipelinePlaceholder({ presence }: { presence: PlaceholderPresence }) {
  return (
    <li
      className={clsx(LIST_SLOT, presence.vanishing && LIST_SLOT_LEAVING)}
      data-testid="pipeline-placeholder"
    >
      <div
        className={clsx(LIST_SLOT_INNER, presence.vanishing && LIST_SLOT_INNER_LEAVING)}
      >
        {/* A real card's silhouette — same material, same padding, same
            neutral edge a guessless row wears — so resolving into a real note
            is a fill-in and not a reflow. It does not lift: it is not
            actionable yet, and a card that offers a press it cannot honour is
            worse than a still one. */}
        <div
          className={clsx(
            "glass-card flex items-center gap-6 border-l-[3px] border-l-ink-faint py-4 pr-5 pl-5",
            LIST_ROW_ENTER,
            LIST_ROW_EXIT,
            presence.vanishing && LIST_ROW_LEAVING,
          )}
        >
          <div role="status" className="min-w-0 flex-1">
            <p
              aria-hidden="true"
              className="flex items-center gap-2.5 text-[15.5px] font-semibold text-ink-dim"
            >
              {/* Ink, never green: green means audio is being recorded, and
                  this whole run happens after a capture has already stopped
                  (docs/DESIGN_SYSTEM.md §2). */}
              <span className="size-[7px] flex-none animate-breathe rounded-full bg-ink-faint" />
              <PhaseStack
                phase={presence.phase}
                transcribing="Transcribing the capture"
                distilling="Distilling the meeting"
              />
            </p>
            <p className="mt-1.5 flex items-center gap-1 font-data text-[10.5px] text-ink-faint tabular-nums">
              <PhaseStack
                phase={presence.phase}
                transcribing="just stopped · queued"
                distilling="reading transcript · routing"
              />
              {/* A running clock, not a phase label: the real transcribe
                  phase alone can hold "Transcribing the capture" for several
                  seconds (model load, ASR, a headless Claude cleanup call),
                  and a ticking number is the honest way to show the run is
                  still alive without claiming to know which of those it's
                  in right now. Inside the aria-hidden layer on purpose: a
                  clock in a live region would announce every second. */}
              <span aria-hidden="true"> · {formatElapsed(presence.elapsedSeconds)}</span>
            </p>
            {/* The announcement itself: one line whose text genuinely
                rewrites as the stage advances, which is what a live region
                actually reacts to — the crossfade above only flips classes,
                and a class flip announces nothing. */}
            <span className="sr-only">
              {presence.phase === "transcribing"
                ? "Transcribing the capture"
                : "Distilling the meeting"}
            </span>
          </div>
        </div>
      </div>
    </li>
  );
}

/**
 * Two texts stacked in one grid cell, crossfading as the phase advances —
 * rather than one line rewriting its content, which would reflow the box at
 * exactly the moment the user is watching it for signs of life.
 *
 * `aria-hidden` because both texts exist at once, which is a lie to anything
 * that reads rather than looks; the placeholder's `sr-only` line is the truth
 * for that reader.
 */
function PhaseStack({
  phase,
  transcribing,
  distilling,
}: {
  phase: PlaceholderPresence["phase"];
  transcribing: string;
  distilling: string;
}) {
  return (
    <span aria-hidden="true" className="grid">
      {(
        [
          ["transcribing", transcribing],
          ["distilling", distilling],
        ] as const
      ).map(([key, text]) => (
        <span
          key={key}
          data-visible={phase === key || undefined}
          className={clsx(
            "col-start-1 row-start-1 transition-opacity duration-200 ease-out-strong",
            "motion-reduce:transition-none",
            phase === key ? "opacity-100" : "opacity-0",
          )}
        >
          {text}
        </span>
      ))}
    </span>
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
      className={clsx(
        "glass-pill focus-ring fixed right-6 bottom-6 z-10 flex cursor-pointer items-center gap-3.5 py-2.5 pr-4 pl-4.5",
        "transition-[translate,opacity,scale] duration-220 ease-out-strong",
        "starting:translate-y-2 starting:opacity-0",
        "not-aria-disabled:active:scale-97 motion-reduce:transition-none",
        // Faster out than in, like every exit in the app (DESIGN_SYSTEM §4).
        toast.fading && "opacity-0 duration-130",
      )}
      onClick={open}
      onMouseEnter={onPause}
      onMouseLeave={onResume}
      onFocus={onPause}
      onBlur={onResume}
      aria-label={`Open the note filed to "${toast.slug}"`}
    >
      <span role="status" className="flex flex-col items-start">
        <span className="font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint">
          Filed to
        </span>
        <span className="mt-0.5 text-[13px] font-semibold text-ink">{toast.slug}</span>
      </span>
      <span className="text-[13px] text-ink-dim" aria-hidden="true">
        →
      </span>
    </button>
  );
}

/**
 * How much of the queue you have cleared, as five lit segments.
 *
 * It sits between the masthead and the list because that is where a progress
 * reading is useful — before you start, not after you scroll. At zero filed it
 * still renders: an unlit meter over a full queue is the honest starting
 * state, and an instrument that only appears once you are winning is one that
 * never helps you begin.
 *
 * Segments rather than a continuous bar because the quantity is discrete —
 * notes cleared, not a percentage of anything — and because a bar that creeps
 * by a pixel per note reads as a loading indicator for work the app is doing,
 * which is exactly backwards here: the work is the user's.
 *
 * Lit segments are INK, never green. Progress is information, and green in
 * this app is the kodama's voice — the system doing something with your words
 * (docs/DESIGN_SYSTEM.md §2). Half strength so the meter reads as a quiet
 * instrument beside the masthead rather than a second headline.
 *
 * `aria-hidden`, not `role="progressbar"`: the masthead summary directly above
 * already says both numbers in words, and announcing this too would report the
 * same fact twice, once as a percentage nobody asked for (§6, one region per
 * concern).
 */
function Progress({ cleared }: { cleared: number }) {
  // Rounded, but never down to nothing while the user is ahead: clearing your
  // first note out of forty has to light something, or the instrument says the
  // work you just did did not count.
  const lit = Math.max(
    cleared > 0 ? 1 : 0,
    Math.round(cleared * METER_SEGMENTS),
  );
  return (
    <div className="mt-4 mb-6 flex gap-1" aria-hidden="true" data-testid="inbox-meter">
      {Array.from({ length: METER_SEGMENTS }, (_, index) => (
        <span
          key={index}
          data-lit={index < lit || undefined}
          className={clsx(
            "h-1 w-11 rounded-[2px] transition-colors duration-200 ease-out-strong",
            index < lit ? "bg-ink/50" : "bg-wash",
          )}
        />
      ))}
    </div>
  );
}

/**
 * One unfiled note, as a card with an action rail.
 *
 * The card's BODY opens the note and the rail beside it disposes of it, and
 * they are plain flex siblings — the old arrangement overlaid the controls on
 * a button that wrapped the whole card, because a button cannot nest a button.
 * The card is no longer a button, so the workaround is gone with it: two
 * regions, each the size of what it does, and no stopPropagation anywhere.
 *
 * The left edge carries the router's guess in that folder's own hue, and the
 * meta line RESTATES it in words (`→ briarwood-golf`). The colour is the fast
 * read, the words are the actual claim; a card that said it in colour alone
 * would say nothing to a third of the people looking at it
 * (docs/DESIGN_SYSTEM.md §6). No confident guess is a faint neutral edge and
 * the words "no confident guess" — an honest blank, not a coin flip painted a
 * colour.
 *
 * The card is the one thing in the app that LIFTS under the pointer. Grove's
 * shape rule is that an action lifts where a destination fills, and this list
 * is the app's one working queue: every card here is a thing to be handled.
 *
 * On success the row plays its exit; the file command broadcasts
 * `vault:changed` itself, which refetches the list and the sidebar count
 * together and drops the row (the file watcher is a fallback for external
 * edits, not the only trigger, so the row leaves even if the watcher never
 * started). A failed file keeps the row and surfaces the message.
 *
 * `fresh` marks the one row that just finished being the pipeline
 * placeholder: it plays a one-shot entrance instead of simply being present,
 * so the placeholder becoming this row reads as continuous rather than as a
 * swap.
 */
function InboxRow({
  note,
  projects,
  onFiled,
  fresh,
}: {
  note: NoteSummary;
  projects: Project[];
  onFiled: () => void;
  fresh: boolean;
}) {
  const { navigate } = useNavigation();
  const [pending, setPending] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [creatingProject, setCreatingProject] = useState(false);

  const guess = suggestion(note);

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
    <li className={clsx(LIST_SLOT, leaving && LIST_SLOT_LEAVING)}>
      <div className={clsx(LIST_SLOT_INNER, leaving && LIST_SLOT_INNER_LEAVING)}>
        <div
          data-testid="inbox-row"
          className={clsx(
            "glass-card flex items-center gap-6 border-l-[3px] py-4 pr-5 pl-5",
            // Named properties only: `translate` and the two the lift recipe
            // sets. A `transition-all` here would put the guess edge's colour
            // on the hover clock too, so a card whose guess changed would
            // fade its edge as if the pointer had done it.
            "transition-[translate,box-shadow,border-color] duration-180 ease-out-strong",
            // `hover:` is redefined in index.css as (hover: hover) and
            // (pointer: fine) — a lift that a touch device cannot undo is a
            // card stuck in a state, so the gating is the variant's, not ours.
            "hover:-translate-y-[2px] hover:glass-card-lift",
            "motion-reduce:transition-none motion-reduce:hover:translate-y-0",
            guess ? GUESS_EDGE[guess.hue] : "border-l-ink-faint",
            LIST_ROW_EXIT,
            leaving && LIST_ROW_LEAVING,
            fresh && LIST_ROW_ENTER,
          )}
        >
          <button
            type="button"
            data-testid="inbox-row-body"
            className="focus-ring-inset min-w-0 flex-1 cursor-pointer rounded-[6px] text-left"
            onClick={() =>
              navigate({
                kind: "noteEditor",
                noteId: note.id,
                project: INBOX_PROJECT,
              })
            }
          >
            {/* Only phrasing content inside a button: the title/meta/snippet
                are block <span>s rather than the <h3>/<p>s the design draws,
                because a button's content model allows nothing more. */}
            <span
              data-testid="inbox-row-title"
              className="block truncate text-[15.5px] font-semibold text-ink"
            >
              {note.title}
            </span>
            <span className="mt-1.5 flex items-center gap-2.5 font-data text-[10.5px] text-ink-faint tabular-nums">
              <span>{[noteKind(note.type), note.date.slice(0, 10)].filter(Boolean).join(" · ")}</span>
              {/* The gauge and the words beside it are one fact twice: the
                  bar is the glance, the percentage is the reading. It is the
                  note's STORED score — why it landed in the Inbox — not the
                  live guess strength, so the card agrees with the file on
                  disk. */}
              <span
                aria-hidden="true"
                className="h-[3px] w-13 overflow-hidden rounded-[2px] bg-wash"
              >
                <span
                  data-testid="inbox-row-gauge"
                  className="block h-full bg-ink-dim"
                  style={{ width: `${Math.round((note.confidence ?? 0) * 100)}%` }}
                />
              </span>
              <span>{matchScore(note.confidence)}</span>
              <span
                data-testid="inbox-row-guess"
                className={clsx("truncate", guess && GUESS_TEXT[guess.hue])}
              >
                {guess ? `→ ${guess.slug}` : "no confident guess"}
              </span>
            </span>
            {note.snippet && (
              <span className="mt-1.5 block truncate text-[13px] text-ink-dim">
                {note.snippet}
              </span>
            )}
          </button>

          {/* The rail: two equal controls in one hairline-separated column,
              so the card's right edge reads as a single place where things
              are decided rather than two loose buttons. */}
          <div className="flex flex-none flex-col items-stretch gap-1.5 self-center border-l border-edge pl-6">
            {/* Always offered, even in a project-less vault: the menu's foot
                creates a project and files into it, so it can no longer be a
                dead end. (It used to be gated on there being somewhere to
                file — that gate was the fix for the dead end, and the foot is
                the better one.) */}
            <Menu.Root>
              <Menu.Trigger
                render={
                  <Button
                    loading={pending}
                    loadingLabel="Filing…"
                    disabled={leaving}
                    aria-label={`File "${note.title}" to project`}
                  >
                    File
                    <span aria-hidden="true" className="text-[10px] text-ink-faint">
                      ▾
                    </span>
                  </Button>
                }
              />
              {/* `end`: the trigger sits at the right edge of the thing it
                  acts on, so the menu hangs back over the card rather than
                  off the panel. */}
              <Menu.Content align="end">
                {guess && (
                  // The suggestion, first and lit. base-ui highlights the
                  // first item when the menu is opened from the keyboard, so
                  // Enter files here — the ↵ is a description of what will
                  // happen, not a decoration. Green is sanctioned on exactly
                  // this: a routing suggestion the system is offering
                  // (DESIGN_SYSTEM §2), in its text-carrying step.
                  <Menu.Item variant="suggested" onClick={() => route(guess.slug)}>
                    <span
                      aria-hidden="true"
                      className={clsx("size-[9px] flex-none rounded-[3px]", HUE_DOT[guess.hue])}
                    />
                    <span className="truncate">{guess.slug}</span>
                    <span className="ml-auto pl-2 font-data text-[10.5px] text-kodama-ink">↵</span>
                  </Menu.Item>
                )}
                {projects
                  .filter((project) => project.slug !== guess?.slug)
                  .map((project) => (
                    // Slugs, not display names: filing is choosing a
                    // location, and a nested project's parentage is the whole
                    // reason you'd pick it over its sibling.
                    <Menu.Item key={project.slug} onClick={() => route(project.slug)}>
                      <span
                        aria-hidden="true"
                        className={clsx(
                          "size-[9px] flex-none rounded-[3px]",
                          HUE_DOT[folderHue(project.slug)],
                        )}
                      />
                      <span className="truncate">{project.slug}</span>
                      <span className="ml-auto pl-2 font-data text-[10.5px] text-ink-faint tabular-nums">
                        {project.note_count}
                      </span>
                    </Menu.Item>
                  ))}
                <Menu.Separator />
                <Menu.Item variant="foot" onClick={() => setCreatingProject(true)}>
                  New project…
                </Menu.Item>
              </Menu.Content>
            </Menu.Root>
            {/* Delete is always offered — even in a project-less vault, where
                there is nowhere to file — because an unwanted note must be
                removable wherever it sits. */}
            <Button
              variant="quiet"
              aria-haspopup="dialog"
              aria-label={`Delete "${note.title}"`}
              disabled={pending || leaving}
              onClick={() => setConfirmingDelete(true)}
            >
              Delete
            </Button>
          </div>
        </div>
        {error && (
          <StatusMessage variant="error" compact>
            Couldn&apos;t file this note: {error}
          </StatusMessage>
        )}
        {confirmingDelete && (
          <DeleteNoteDialog
            id={note.id}
            noteTitle={note.title}
            sessionBacked={isSessionSource(note.source)}
            onClose={() => setConfirmingDelete(false)}
            // Mirror the file flow: close, then play the row's exit. The delete
            // command broadcasts `vault:changed`, which refetches the list and
            // the sidebar Inbox count together and drops the row.
            onDeleted={() => {
              setConfirmingDelete(false);
              setLeaving(true);
            }}
          />
        )}
        {creatingProject && (
          <CreateProjectDialog
            onClose={() => setCreatingProject(false)}
            // The errand was "file this note somewhere new", so creating the
            // project finishes it rather than navigating away and leaving the
            // note where it was.
            onCreated={(project) => {
              setCreatingProject(false);
              route(project.slug);
            }}
          />
        )}
      </div>
    </li>
  );
}
