import { clsx } from "clsx";
import { AnimatePresence, motion, type Variants } from "motion/react";
import { useMemo, useState } from "react";
import {
  pipelineStage,
  savedDestination,
  savedPathMatchesNote,
  transcribeBeat,
  useCapturePipeline,
  type CapturePipeline,
  type PipelineStage,
  type TranscribeBeat,
} from "../../useCapturePipeline";
import { useNavigation } from "../../useNavigation";
import { matchScore, noteKind } from "../../noteMeta";
import {
  CATEGORY_OPTIONS,
  categoryLabel,
  fileNoteToProject,
  INBOX_PROJECT,
  listNotes,
  setNoteCategory,
  useProjectNotes,
  type NoteCategory,
  type NoteSummary,
} from "../../useNotes";
import { folderHue, useProjects, type FolderHue, type Project } from "../../useProjects";
import { isSessionSource } from "../../useSessions";
import { backendCopy } from "../../errorCopy";
import { formatElapsed, useElapsed } from "../../useElapsed";
import { useReduceMotion } from "../../useReduceMotion";
import { useTimeout } from "../../useTimeout";
import { SpiritMark } from "../capture/SpiritMark";
import { CreateProjectDialog } from "../dialogs/CreateProjectDialog";
import { DeleteNoteDialog } from "../dialogs/DeleteNoteDialog";
import { Button } from "../ui/Button";
import { Menu } from "../ui/Menu";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

/** How long the placeholder's travel-left-and-vanish plays once distill routes
 * it to a project: the card clears in 220ms inside a slot that closes in 280,
 * so this is the slot's clock — the moment there is nothing left to see. */
const VANISH_MS = 280;
/** How long "Filed to <project>" stays up before it goes — the same dwell
 * `CaptureToast` used to give a success notice. */
const TOAST_DWELL_MS = 3500;
/** The app's Exit band (110–130ms, docs/DESIGN_SYSTEM.md §4): the toast's own
 * fade-out, once the dwell ends. It is shorter than the entrance it undoes,
 * which is the rule for every exit in the app. Spent by the exit variant rather
 * than by a timer — `AnimatePresence` holds the element alive for exactly this
 * long. */
const TOAST_FADE_S = 0.13;
/** The toast's arrival: the app's Materialize band, which every surface that
 * arrives shares (docs/DESIGN_SYSTEM.md §4). Longer than the fade above, which
 * is the rule for every entrance/exit pair in the app. */
const TOAST_MATERIALIZE_S = 0.22;
/** Covers the row's own fill-in with a little room: how long after an
 * Inbox-routed note lands before its outcome counts as fully presented — the
 * fill-in has played and there is nothing left for a remount to replay. */
const FILL_IN_MS = 200;
/** Covers the three ways a stage could otherwise wait forever: a stop the
 * backend never acknowledges (a mis-tap under its minimum session duration
 * emits nothing at all), a dev/mock build never following a saved transcript
 * with a distill, and an Inbox-routed save whose vault refetch fails or
 * never lands. */
const GRACE_MS = 10_000;

/*
 * THE MOTION OF A ROW IN THIS LIST.
 *
 * The `motion` package drives it rather than CSS, and this list is why the
 * package is in the stack at all: a row leaving has to collapse a height nobody
 * measured, while its neighbours glide up to meet it, while a new row may be
 * arriving into the same list — three things on one timeline, interruptible
 * halfway through. CSS can express any one of them and none of them together.
 * Everything else in the app stays CSS (docs/UI_CONVENTIONS.md).
 *
 * Two elements per row, and the split is load-bearing: the SLOT (the `li`) owns
 * the space, the CARD inside it owns the travel. Collapsing the space and
 * sliding the card are different clocks on purpose — the card is gone at 220ms
 * while the slot is still closing at 280, so the space hands itself back behind
 * a card that has already left, and the row below never appears to shove it
 * out. They are one movement because they overlap, not because they match.
 *
 * The variants are keyed by label, so the slot and the card can be told
 * "you are leaving" once, on the parent, and each answer in its own values.
 *
 * TWO THINGS THIS DELIBERATELY DOES NOT USE, both of which the ticket named.
 *
 * No `layout` prop. The neighbours glide because the slot's HEIGHT is animating
 * and they are in normal flow behind it — the same trick the prototype used,
 * and it needs no projection. `layout` would put every row in a projection tree
 * that writes its own transforms, fighting the card's `x` underneath it: two
 * writers of one property. It earns its keep when items REORDER, and nothing
 * here reorders. Rows only arrive and leave.
 *
 * No `exit` variant on the list's children either, and this one is a real
 * constraint rather than a preference. A departure here is always something the
 * app already knows about BEFORE the element unmounts — `vanishing` on the
 * placeholder, `leaving` on a row — because both start the moment the command
 * succeeds rather than whenever the vault refetch happens to land. Animating
 * from that state is both earlier and more honest than animating on the way
 * out. `exit` would also make each row's unmount wait on an animation
 * completing inside a subtree that holds a base-ui menu and two dialogs, and a
 * row that can fail to unmount is a far worse bug than a collapse that gets
 * clipped. `AnimatePresence` is kept for the filed toast, whose departure genuinely
 * has no state to animate from: it is dropped and then gone.
 *
 * The placeholder's silent handoff is the case that proves the rule — see the
 * comment on its `motion.li`.
 *
 * Each table is one FROZEN module constant per reduced-motion setting, picked
 * by a getter, rather than an object literal built during render. A variants
 * object is part of a `motion` element's identity: handing it a structurally
 * identical but newly-allocated one on every render is a change as far as the
 * animator is concerned, and it re-reads the whole table each time — on a list
 * that re-renders on every vault refetch, every keystroke in a dialog and
 * every menu open.
 */

/** `--ease-out-strong`, as numbers: entrances and arrivals. */
const EASE_OUT_STRONG = [0.23, 1, 0.32, 1] as const;
/** `--ease-in-out-strong`, as numbers: departures, which leave under power. */
const EASE_IN_OUT_STRONG = [0.77, 0, 0.175, 1] as const;

/** The gap between rows, in px. It lives on each slot as an animated margin
 * rather than on the list as a flex `gap`, because a gap cannot collapse: a
 * row whose height reaches zero with a live gap under it still holds 14px of
 * the list open, and the neighbours land with a jump at the end of a
 * choreography built to avoid exactly that. */
const ROW_GAP = 14;

/**
 * The slot: the space a row occupies, and the only thing that ever animates a
 * height. `height: "auto"` is motion measuring the row for us — the same
 * problem the `1fr → 0fr` grid trick used to solve, minus the grid.
 *
 * `overflow: "clip"` is set (not animated) at the start of the exit, so the
 * clipping exists only while collapsing. A resting row must never be clipped:
 * it would crop its own hover lift and, worse, its focus ring, in the densest
 * list in the app.
 */
const SLOT_MOVING: Variants = {
  enter: { height: 0, opacity: 0, marginBottom: 0 },
  rest: {
    height: "auto",
    opacity: 1,
    marginBottom: ROW_GAP,
    // Rise-in, the band for a row appearing (docs/DESIGN_SYSTEM.md §4), and
    // the same 280 the collapse below runs on.
    transition: { duration: 0.28, ease: EASE_OUT_STRONG },
  },
  gone: {
    height: 0,
    marginBottom: 0,
    overflow: "clip",
    transition: { duration: 0.28, ease: EASE_IN_OUT_STRONG },
  },
};
/** Fades only. No height: the collapse IS the movement, and the rule is that
 * movement goes while life stays (docs/DESIGN_SYSTEM.md §4).
 *
 * The margin is NOT movement, though, and it is not optional either: it is the
 * list's 14px gap, which lives here rather than on the `ul` so it can collapse
 * with the row that owns it. Every state carries the same value, so it is set
 * once and never animated — leave it out of one of them and the rows go flush
 * the moment the preference is on. */
const SLOT_STILL: Variants = {
  enter: { opacity: 0, marginBottom: ROW_GAP },
  rest: { opacity: 1, marginBottom: ROW_GAP, transition: { duration: 0.2 } },
  gone: {
    opacity: 0,
    marginBottom: ROW_GAP,
    transition: { duration: TOAST_FADE_S },
  },
};

function slotVariants(reduce: boolean): Variants {
  return reduce ? SLOT_STILL : SLOT_MOVING;
}

/**
 * The card: what the eye actually follows. Out to the LEFT, because that is
 * where this list came from — a row leaves along the axis it arrived on rather
 * than picking a new direction on the way out — carrying a 2px blur, which is
 * the part that makes it read as departing rather than as being deleted.
 */
/* The blur appears ONLY in `gone`, never in the resting states, and that is a
   correctness point rather than a tidiness one: a `filter` in the resting
   variant would sit in the inline style of every card in the list forever, and
   a filtered element is a containing block for fixed-position descendants and
   its own stacking context. Each row holds a menu and two dialogs that position
   against the viewport, so a resting `blur(0px)` would quietly re-anchor them
   to the card. Motion reads the computed value as the start of the exit, which
   is what it should be animating from anyway. */
const CARD_MOVING: Variants = {
  enter: { opacity: 0, x: 0 },
  rest: { opacity: 1, x: 0 },
  gone: {
    opacity: 0,
    x: -24,
    filter: "blur(2px)",
    transition: { duration: 0.22, ease: EASE_IN_OUT_STRONG },
  },
};
const CARD_STILL: Variants = {
  enter: { opacity: 0 },
  rest: { opacity: 1 },
  gone: { opacity: 0, transition: { duration: TOAST_FADE_S } },
};

function cardVariants(reduce: boolean): Variants {
  return reduce ? CARD_STILL : CARD_MOVING;
}

/* The folder hues, written out rather than interpolated: Tailwind cannot see a
   constructed class name and would emit none of these
   (docs/UI_CONVENTIONS.md). The card edge, the guess words and the menu dot all
   read from the same three tables, so a project is one colour wherever it
   appears. */
/* The edge states the hue twice, once for rest and once for hover, and the
   second one is not redundant. `glass-card-lift` brightens the card's whole
   `border-color`, and it is a `:hover` rule — (0,2,0) against a bare
   `border-l-*`'s (0,1,0) — so it outranks the resting hue on specificity
   whatever order Tailwind emits them in. Without the hover restatement the
   guess colour drops off the edge exactly while the pointer is on the card:
   the fast read disappearing at the moment it is being read. Checked in the
   built CSS, which is the only place this kind of failure is visible
   (docs/UI_CONVENTIONS.md §6). */
const GUESS_EDGE: Record<FolderHue, string> = {
  coral: "border-l-coral hover:border-l-coral",
  cobalt: "border-l-cobalt hover:border-l-cobalt",
  teal: "border-l-teal hover:border-l-teal",
  plum: "border-l-plum hover:border-l-plum",
};
/** The same restatement for a card with nothing to suggest: its neutral edge is
 * a content choice too, and `glass-card-lift` would repaint it the material's
 * edge colour on hover. */
const NEUTRAL_EDGE = "border-l-ink-faint hover:border-l-ink-faint";
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
/** The toast's left edge, in the hue of the folder the note landed in — the
 * same fast read the card carried, kept through the handoff, so the thing that
 * announces where a note went is the colour of where it went. No hover
 * restatement here: nothing repaints this surface's border. */
const TOAST_EDGE: Record<FolderHue, string> = {
  coral: "border-l-coral",
  cobalt: "border-l-cobalt",
  teal: "border-l-teal",
  plum: "border-l-plum",
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
  // Read here as well as at the shell's `MotionConfig`, because that setting
  // covers the transforms and the layout animations and this list also
  // animates a height and a blur — neither of which "reduced motion" turns off
  // on its own, and both of which are exactly the choreography to drop.
  const reduce = useReduceMotion();

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
        <StatusMessage variant="error">{error}</StatusMessage>
      ) : (
        <>
          {remaining > 0 && <Progress cleared={cleared} />}
          {remaining === 0 && !placeholder ? (
            !loading && <EmptyInbox />
          ) : (
            // `mt-[26px]` is the body's own header gap: `ViewFrame`'s head
            // carries no bottom margin, so each view opens its body at this
            // step (ChatView and NeedsAttentionView both do). The meter used to
            // supply it here by accident, and it only renders above zero — so
            // with nothing unfiled and a placeholder showing, the card sat
            // flush under the title. In the populated state this collapses with
            // `Progress`'s `mb-6` rather than adding to it.
            //
            // No flex `gap`: the 14px is an animated margin on each slot, so
            // it collapses with the row that owns it (see `ROW_GAP`).
            <ul data-testid="inbox-list" className="mt-[26px] flex flex-col">
              {placeholder && <PipelinePlaceholder presence={placeholder} reduce={reduce} />}
              {notes.map((note) => (
                // Keyed by path, not id: two files can carry the same id (an
                // external copy), and duplicate keys would mis-reconcile rows.
                <InboxRow
                  key={note.path}
                  note={note}
                  projects={projects}
                  onFiled={() => setFiledThisSession((count) => count + 1)}
                  fresh={note.path === freshNotePath}
                  reduce={reduce}
                />
              ))}
            </ul>
          )}
        </>
      )}
      {/* Its own presence group: the toast is not in the list, and it has to
          be able to arrive while the placeholder that summoned it is still
          leaving. */}
      <AnimatePresence>
        {toast && (
          <FiledToast toast={toast} onPause={pauseToast} onResume={resumeToast} reduce={reduce} />
        )}
      </AnimatePresence>
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
    <div className="flex flex-col items-center gap-1.5 pt-24 pb-[70px] text-center">
      {/* The mark's own `idle` mode carries the dormant ink step, so this needs
          no per-screen dimming (src/index.css §3). */}
      <SpiritMark mode="idle" size="26px" className="mb-[18px]" />
      <p className="text-[15px] font-semibold text-ink">Nothing waiting.</p>
      {/* Dim, not faint: `ink-faint` is the metadata register and is spent on
          timestamps, counts and ids (docs/DESIGN_SYSTEM.md §6). This is a
          sentence the user reads for meaning, which is the one thing that
          register may not carry. */}
      <p className="max-w-[44ch] text-[12.5px] leading-[1.55] text-ink-dim">
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
 * what the transcribing phase has to say for itself, and how long this run
 * has been going.
 *
 * The transcribe phase spans a model load, the ASR pass, and a headless
 * Claude cleanup call. The ASR pass is the long one and now reports its real
 * position in the recording (`transcribing.fraction`), which is what the bar
 * and the "12:30 of 58:04" figures draw on; the stages either side of it have
 * no fraction and say so in words instead. The clock stays regardless: it is
 * the one signal that keeps proving the run is alive across all three, and it
 * never has to claim which one it is in. */
type PlaceholderPresence = {
  phase: "transcribing" | "distilling";
  vanishing: boolean;
  transcribing: TranscribeBeat;
  elapsedSeconds: number;
};

/** The one success announcement left after `CaptureToast` went
 * failures-only: a note filed to a different project. Inbox-owned rather
 * than shared with `CaptureToast`, so that component's failures-only
 * contract stays exactly what it says.
 *
 * No `fading` flag: the toast's exit belongs to `AnimatePresence` now, which
 * keeps the element alive for the length of its own exit variant. Dropping it
 * from state is the whole of dismissing it. */
type FiledToastPresence = { slug: string; savedPath: string };

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
  const [toast, setToast] = useState<{ id: string; slug: string; savedPath: string } | null>(null);
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

  // The toast rises on the same frame the placeholder starts leaving, rather
  // than waiting for it to finish. That overlap is the point: the eye follows
  // the card out to the left and finds the answer already arriving in the
  // corner, with no beat in between where the app has nothing to say. Adjusted
  // during render rather than from the vanish timer
  // (.claude/rules/no-use-effect.md), and guarded on the stage id so a
  // re-render cannot re-open a toast the user already dismissed.
  if (vanishing && resolved?.kind === "filedToProject" && toast?.id !== resolved.id) {
    setToastPaused(false);
    setToast({ id: resolved.id, slug: resolved.slug, savedPath: resolved.savedPath });
  }

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
  // for good — marked handled at the shell, so a remount cannot replay it.
  // The toast is already up by now (opened at the start of the vanish, above);
  // all this does is retire the row that handed off to it.
  useTimeout(
    () => {
      if (!resolved || resolved.kind !== "filedToProject") return;
      pipeline.markFiledHandled(resolved.id);
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

  // The toast's own lifecycle, now one timer instead of two: dwell (paused
  // while the user is reading it or on their way to click it), then drop it
  // from state. The fade that used to need its own flag and its own timer is
  // the exit variant's, played by `AnimatePresence` after this.
  useTimeout(
    () => setToast(null),
    toast && !toastPaused ? TOAST_DWELL_MS : null,
    toast?.id ?? null,
  );

  const placeholder: PlaceholderPresence | null =
    visible && resolved
      ? {
          phase: resolved.kind === "transcribing" ? "transcribing" : "distilling",
          vanishing,
          transcribing: transcribeBeat(pipeline.transcription),
          elapsedSeconds,
        }
      : null;

  return {
    placeholder,
    toast: toast ? { slug: toast.slug, savedPath: toast.savedPath } : null,
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
 * `phase` advances rather than the line swapping out from under itself — so
 * the box never reflows. Within the transcribing half, the meta text does
 * rewrite (the clock every second, the progress figures every chunk), which
 * costs nothing: it is one grid cell of `tabular-nums`, so digits change
 * without moving. The progress bar likewise takes no space of its own, riding
 * the card's bottom edge and mounted the whole time.
 *
 * Both texts existing at once (and the once-a-second clock) is a visual trick,
 * though, so the whole visual layer is `aria-hidden` and the `role="status"`
 * region announces through one sr-only line that genuinely rewrites as the
 * stage advances (docs/DESIGN_SYSTEM.md §6) — a class flip alone would
 * announce nothing, and a live clock or a live fraction would announce
 * forever.
 */
function PipelinePlaceholder({
  presence,
  reduce,
}: {
  presence: PlaceholderPresence;
  reduce: boolean;
}) {
  return (
    <motion.li
      data-testid="pipeline-placeholder"
      variants={slotVariants(reduce)}
      // It grows in — the one deliberate entrance the app spends on
      // distill-and-route — where a real row arriving simply appears.
      initial="enter"
      // The vanish plays HERE, while the element is still mounted, and the
      // state machine holds it mounted for exactly as long as it takes
      // (VANISH_MS). That is what makes an exit variant not merely
      // unnecessary but wrong: this element unmounts down two paths and only
      // one of them is a departure. Routed to a project, it leaves — the
      // slide-and-collapse above. Routed to the Inbox, it unmounts the instant
      // the real row appears, and that has to be SILENT, because the row
      // taking its place is the same note. An exit there would turn a fill-in
      // into a swap, and the user would watch their note leave and arrive
      // instead of resolve. `exit` cannot tell those two apart. This can.
      animate={presence.vanishing ? "gone" : "rest"}
    >
      <motion.div variants={cardVariants(reduce)} className="min-h-0">
        {/* A real card's silhouette — same material, same padding, same
            neutral edge a guessless row wears — so resolving into a real note
            is a fill-in and not a reflow. It does not lift: it is not
            actionable yet, and a card that offers a press it cannot honour is
            worse than a still one. */}
        <div className="glass-card relative flex items-center gap-6 overflow-hidden border-l-[3px] border-l-ink-faint py-4 pr-5 pl-5">
          <div role="status" className="min-w-0 flex-1">
            <p
              aria-hidden="true"
              className="flex items-center gap-2.5 text-[15.5px] font-semibold text-ink-dim"
            >
              {/* Ink, never green: green means audio is being recorded, and
                  this whole run happens after a capture has already stopped
                  (docs/DESIGN_SYSTEM.md §2). */}
              {/* `animate-pending`, not `animate-breathe`: breathe is the
                  kodama's, a pure transform whose reduced-motion partner is
                  nothing at all, so this dot went still exactly when it was
                  still working. Pending is opacity-only and is the animation
                  src/index.css names for this dot. */}
              <span className="size-[7px] flex-none animate-pending rounded-full bg-ink-faint" />
              <PhaseStack
                phase={presence.phase}
                transcribing="Transcribing the capture"
                distilling="Distilling the meeting"
              />
            </p>
            <p className="mt-1.5 flex items-center gap-1 font-data text-[10.5px] text-ink-faint tabular-nums">
              {/* The transcribing half names what is actually happening —
                  "queued" is kept for the one run that genuinely is, parked
                  behind another on `TRANSCRIBE_LOCK` (see `transcribeBeat`).
                  Still one string, so the crossfade below fires on the phase
                  change alone; the figures inside it rewrite in place, exactly
                  as the clock does. */}
              <PhaseStack
                phase={presence.phase}
                transcribing={
                  presence.transcribing.figures
                    ? `${presence.transcribing.copy} · ${presence.transcribing.figures}`
                    : presence.transcribing.copy
                }
                distilling="reading transcript · routing"
              />
              {/* A running clock, not a phase label: the transcribe phase
                  spans a model load, the ASR pass and a headless Claude
                  cleanup call, and only the middle one has a fraction to
                  report — a ticking number is what keeps proving the run is
                  alive across all three without claiming to know which it is
                  in. Inside the aria-hidden layer on purpose: a clock in a
                  live region would announce every second. */}
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
          {/* The ASR pass's real position in the recording. Ink, never green,
              for the same reason the dot above is (docs/DESIGN_SYSTEM.md §2),
              and continuous rather than segmented because audio seconds are a
              genuinely continuous quantity — the reasoning
              `ModelDownloadProgress` sets out at length, one screen over.
              No `role="progressbar"`: the figures are already stated in the
              meta line, and this card's live region carries phase words only
              (§6, one region per concern) — a fraction announcing itself every
              chunk would make it unusable.

              It rides the card's bottom edge rather than taking a row of its
              own, and it is mounted for the placeholder's whole life with only
              its opacity changing. Both are the same promise: this card wears
              a real row's exact silhouette, so resolving into the real note
              stays a fill-in and neither the first progress event nor the
              distilling crossfade can reflow it.

              Local rather than a `src/components/ui/` primitive: the two divs
              are all it shares with `ModelDownloadProgress` — the height, the
              reserved space, the absence of a zero-state and of a header row
              all differ — so extraction stays deferred (UI_CONVENTIONS §4). */}
          <div
            aria-hidden="true"
            className={clsx(
              "absolute inset-x-0 bottom-0 h-[3px] bg-wash",
              "transition-opacity duration-200 ease-out-strong motion-reduce:transition-none",
              presence.phase === "transcribing" && presence.transcribing.fraction !== null
                ? "opacity-100"
                : "opacity-0",
            )}
          >
            <div
              data-testid="placeholder-progress-fill"
              className={clsx(
                "h-full bg-ink/50",
                "transition-[width] duration-200 ease-out-strong motion-reduce:transition-none",
              )}
              style={{ width: `${(presence.transcribing.fraction ?? 0) * 100}%` }}
            />
          </div>
        </div>
      </motion.div>
    </motion.li>
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
 *
 * A 2px blur bridges the swap: the outgoing text softens as it goes and the
 * incoming one resolves as it arrives, so the two read as one thing changing
 * rather than as two things overlapping. Deliberately still CSS — a crossfade
 * between two known states is what a transition is for, and the `motion`
 * package earns its place on this screen for the list choreography it drives,
 * not for everything that moves (docs/UI_CONVENTIONS.md).
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
            "col-start-1 row-start-1 transition-[opacity,filter] duration-200 ease-out-strong",
            "motion-reduce:transition-none",
            phase === key ? "opacity-100 blur-none" : "opacity-0 blur-[2px]",
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
 *
 * It rises out of the corner it lives in, which is the app's rule for a
 * surface materializing (docs/DESIGN_SYSTEM.md §4): up, up to full size, and
 * out of a 3px blur, all from a bottom-right origin — so it reads as having
 * come from that corner rather than having been placed there. Its left edge
 * carries the hue of the folder the note landed in, the same fast read the
 * card wore on its way out.
 */
function FiledToast({
  toast,
  onPause,
  onResume,
  reduce,
}: {
  toast: FiledToastPresence;
  onPause: () => void;
  onResume: () => void;
  reduce: boolean;
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
            ? {
                kind: "noteEditor",
                noteId: note.id,
                project: toast.slug,
                origin: { kind: "inbox" },
              }
            : { kind: "project", slug: toast.slug },
        );
      })
      .catch(() => navigate({ kind: "project", slug: toast.slug }));
  };

  return (
    <motion.button
      type="button"
      className={clsx(
        // `fixed`, not absolute inside the panel: the panel scrolls, and a
        // toast that scrolled with the list it is reporting on would be an
        // announcement you could lose by moving. The page gutter is 5 and this
        // inset is 6, so it still lands just inside the panel's own corner.
        "glass-overlay focus-ring fixed right-6 bottom-6 z-10 flex cursor-pointer items-center gap-3.5",
        "rounded-card border-l-[3px] py-2.5 pr-4 pl-4.5",
        TOAST_EDGE[folderHue(toast.slug)],
      )}
      // Bottom right, because that is the corner it occupies: a surface
      // materializes from its own origin, not from the middle of the screen.
      style={{ transformOrigin: "bottom right" }}
      initial={reduce ? { opacity: 0 } : { opacity: 0, y: 12, scale: 0.97, filter: "blur(3px)" }}
      animate={
        reduce ? { opacity: 1 } : { opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }
      }
      // Faster out than in, like every exit in the app (DESIGN_SYSTEM §4), and
      // a plain fade: it is being dismissed, not travelling anywhere.
      exit={{ opacity: 0, transition: { duration: TOAST_FADE_S } }}
      // Materialize: 220ms, the band for a surface arriving
      // (docs/DESIGN_SYSTEM.md §4). It is the toast's own clock, not the
      // list's — the choreography next door runs longer because it is showing
      // work, and a toast is not.
      transition={{ duration: TOAST_MATERIALIZE_S, ease: EASE_OUT_STRONG }}
      // The app's one press spec. `whileTap` rather than `active:scale-97`
      // because motion owns this element's transform while it is arriving, and
      // a CSS `scale` landing mid-entrance would fight it. Its own 140ms
      // (DESIGN_SYSTEM §2) rather than the entrance's, which the element-level
      // `transition` above would otherwise lend it.
      whileTap={{ scale: 0.97, transition: { duration: 0.14, ease: EASE_OUT_STRONG } }}
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
    </motion.button>
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
  reduce,
}: {
  note: NoteSummary;
  projects: Project[];
  onFiled: () => void;
  fresh: boolean;
  reduce: boolean;
}) {
  const { navigate } = useNavigation();
  const [pending, setPending] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [creatingProject, setCreatingProject] = useState(false);

  const guess = suggestion(note);

  /** Correcting a meeting's genre. Unlike `route` this sets no `leaving`: the
   *  note stays in the Inbox — a kind is not a filing — so the row repaints in
   *  place off the write's own `vault:changed` broadcast. */
  const recategorize = (category: NoteCategory) => {
    if (category === note.category) return;
    setPending(true);
    setError(null);
    setNoteCategory({ id: note.id, category })
      .catch((err: unknown) => {
        setError(
          backendCopy(err, "Couldn't change this meeting's kind. The note is unchanged; try again."),
        );
      })
      .finally(() => setPending(false));
  };

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
        setError(
          backendCopy(err, "Couldn't file this note. It is still in the Inbox; try again."),
        );
      });
  };

  return (
    <motion.li
      variants={slotVariants(reduce)}
      // Only the row that just stopped being the placeholder plays an
      // entrance — that IS the fill-in, and it is why the handoff reads as one
      // note resolving rather than as a swap. Every other row is simply
      // already there.
      initial={fresh ? "enter" : false}
      // `leaving` starts the departure the moment the command succeeds, rather
      // than waiting for the vault refetch to drop the row: the click has to
      // be answered now, and the refetch lands whenever it lands. By the time
      // it does, the row is already at these values and the unmount is silent.
      animate={leaving ? "gone" : "rest"}
    >
      <motion.div variants={cardVariants(reduce)} className="min-h-0">
        <div
          data-testid="inbox-card"
          data-fresh={fresh || undefined}
          className={clsx(
            "glass-card flex items-center gap-6 border-l-[3px] py-4 pr-5 pl-5",
            // `hover:` is redefined in index.css as (hover: hover) and
            // (pointer: fine) — a lift that a touch device cannot undo is a
            // card stuck in a state, so the gating is the variant's, not ours.
            "hover:-translate-y-[2px] hover:glass-card-lift",
            "motion-reduce:hover:translate-y-0",
            guess ? GUESS_EDGE[guess.hue] : NEUTRAL_EDGE,
            // The card's only transition, now that entering and leaving belong
            // to motion: the hover lift. Named properties only — a
            // `transition-all` here would put the guess edge's colour on the
            // hover clock too, so a card whose guess changed would fade its
            // edge as if the pointer had done it.
            "transition-[translate,box-shadow,border-color] duration-180 ease-out-strong",
            "motion-reduce:transition-none",
          )}
        >
          {/* `inbox-row` names the OPENING control, not the card: the e2e
              harness's `clickRowLabelled` (e2e/lib/page.mjs) calls `.click()`
              directly on the element carrying this testid, and a native click
              dispatches at its target and bubbles up — never down into a
              child — so the testid has to sit on the thing that is actually
              clickable. The outer card is `inbox-card`. */}
          <button
            type="button"
            data-testid="inbox-row"
            className="focus-ring-inset min-w-0 flex-1 cursor-pointer rounded-[6px] text-left"
            onClick={() =>
              navigate({
                kind: "noteEditor",
                noteId: note.id,
                project: INBOX_PROJECT,
                origin: { kind: "inbox" },
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
              <span>
                {[
                  noteKind(note.type),
                  note.category ? categoryLabel(note.category) : null,
                  note.date.slice(0, 10),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
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
                  /* One inert form, not two. The success path sets `leaving`
                     without clearing `pending`, so passing both put the native
                     `disabled` back on a control that was mid-task — and
                     `disabled` wins over `loading`, which drops focus to the
                     document body just as base-ui returns it to this trigger.
                     That is the exact bug `loading` exists to avoid
                     (docs/UI_CONVENTIONS.md §4). Busy through the departure
                     keeps it focusable, and `Button` swallows the pointer and
                     arrow activation base-ui opens this menu on — not just the
                     click — so the menu cannot reopen over a note that has
                     already moved. */
                  <Button
                    loading={pending || leaving}
                    loadingLabel="Filing…"
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
                {/* The row's affordance ceiling is two (UI_CONVENTIONS §5) and
                    filing and Delete already spend both, so the genre folds in
                    here rather than growing a third control — and it belongs
                    here, because the slot holds this row's verdicts about this
                    note, which is what a kind is. It cannot ride the meta line
                    instead: that text sits inside the row's own open-note
                    <button>, and a button inside a button is invalid. */}
                {note.type === "meeting" && (
                  <>
                    <Menu.Separator />
                    <Menu.Item variant="foot" disabled>
                      Meeting kind
                    </Menu.Item>
                    {CATEGORY_OPTIONS.map((option) => (
                      <Menu.Item
                        key={option.value}
                        onClick={() => recategorize(option.value)}
                      >
                        <span className="truncate">{option.label}</span>
                        {option.value === note.category && (
                          <span
                            aria-hidden="true"
                            className="ml-auto pl-2 font-data text-[10.5px]"
                          >
                            ✓
                          </span>
                        )}
                      </Menu.Item>
                    ))}
                  </>
                )}
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
            {error}
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
      </motion.div>
    </motion.li>
  );
}
