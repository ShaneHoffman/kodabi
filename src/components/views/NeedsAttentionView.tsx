import { useState } from "react";
import { DISTILL_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import {
  deleteSession,
  dismissSession,
  restoreSession,
  retryDistill,
  useFailedSessions,
  type FailedSession,
} from "../../useSessions";
import { notifyVaultChanged } from "../../useVaultQuery";
import type { DistillEvent } from "../../useDistillState";
import { SpiritMark } from "../capture/SpiritMark";
import { Button } from "../ui/Button";
import { DestructiveConfirmDialog } from "../ui/DestructiveConfirmDialog";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

/**
 * The card both stacks are built from.
 *
 * THE CARDS ARE ALREADY LIFTED, and that is the inverse of the Inbox. An Inbox
 * row is flat until you touch it, because you are the one deciding to act on
 * it. These items flagged THEMSELVES: something failed while you were not
 * looking, and the card is off the page before you arrive to say so. There is
 * no hover state at all — the plane is not an affordance here, it is a
 * statement about the item (docs/UI_CONVENTIONS.md §4).
 *
 * The left edge is the caller's: marigold on a live failure, faint ink on a
 * dismissed one. Marigold is failure and only failure, which is why it is not
 * in the folder palette (docs/DESIGN_SYSTEM.md §2) — and why it never carries
 * the meaning alone, the badge and the message line say the same thing in ink.
 */
const ATTENTION_CARD_CLASSES = "glass-card flex items-center gap-6 border-l-[3px] px-5 py-4";

/** A readable name for a session: its slug de-hyphenated, else an honest
 * placeholder. Never the capture time, which the line below already shows, and
 * never the filename — a session captured without a slug is named
 * `20260719T190540729Z-5fonvqzd.jsonl`, and setting a machine id in the reading
 * serif was the single ugliest thing on the screen. */
function sessionTitle(session: FailedSession): string {
  const slug = session.slug?.split("-").filter(Boolean).join(" ").trim();
  return slug || "Untitled capture";
}

/**
 * The device-ID segment of a session filename — the `{timestamp}-{deviceID}`
 * prefix `kodabi_core::naming` writes — which is the closest thing a capture
 * has to a short id a person can quote back when something goes wrong. Null
 * for a hand-imported name that doesn't follow the scheme, so the meta line
 * shows the time alone rather than a misparsed fragment.
 */
function sessionShortId(session: FailedSession): string | null {
  const stem = session.file_name.replace(/\.jsonl$/, "");
  const deviceId = stem.split("-")[1];
  // 8 lowercase base36 characters, matching `DeviceId::parse`.
  return deviceId && /^[0-9a-z]{8}$/.test(deviceId) ? deviceId : null;
}

/** The stored instant (UTC) rendered in the user's own zone, since this is a
 * timestamp a person reads rather than one anything sorts by. */
function formatCaptureTime(capturedAt: string): string {
  const parsed = new Date(capturedAt);
  if (Number.isNaN(parsed.getTime())) return capturedAt;
  return parsed.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/** The most recent failure, as a short date, for the header's state line. */
function lastFailure(sessions: FailedSession[]): string | null {
  const newest = sessions
    .map((session) => new Date(session.captured_at).getTime())
    .filter((time) => !Number.isNaN(time))
    .sort((a, b) => b - a)[0];
  if (newest === undefined) return null;
  return new Date(newest).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/**
 * The body column both card kinds share: title, mono meta, and — on a live
 * failure — the marigold badge and the plain-language line under it.
 *
 * A dismissed card mutes its title and drops both: it is still readable, and
 * no longer asking for anything. Saying "no note created" beside a capture the
 * user has already waved off would be the screen re-raising a settled thing.
 */
function CaptureSummary({
  session,
  rowError,
  dismissed,
}: {
  session: FailedSession;
  rowError?: string;
  dismissed?: boolean;
}) {
  const shortId = sessionShortId(session);
  return (
    <div className="min-w-0 flex-1">
      <p className={`text-[15.5px] font-semibold ${dismissed ? "text-ink-dim" : "text-ink"}`}>
        {sessionTitle(session)}
      </p>
      <div className="mt-1.5 flex items-center gap-2.5 font-data text-[10.5px] tabular-nums text-ink-faint">
        <span>
          {shortId && `session ${shortId} · `}
          {formatCaptureTime(session.captured_at)}
        </span>
        {/* Marigold text, which day mode measures at 3.67:1 — under the reading
            floor, and allowed only because nothing depends on reading it. The
            card's edge, this badge, and the sentence below all say the same
            thing, so the badge is the redundant encoding, never the sole one
            (docs/DESIGN_SYSTEM.md §6). */}
        {!dismissed && <span className="text-warn">no note created</span>}
      </div>
      {!dismissed && (
        // What actually happened, in the user's terms rather than the
        // pipeline's. The backend keeps no failure reason — a session lands
        // here because nothing claimed it — so this says the one thing that is
        // true of every path here, and says the recording survived.
        <p className="mt-[7px] text-[13px] text-ink-dim">
          Kodabi made no note from this capture. The recording is safe on disk,
          and Retry runs it again.
        </p>
      )}
      {rowError && (
        <StatusMessage variant="error" compact>
          {rowError}
        </StatusMessage>
      )}
    </div>
  );
}

/** A marker action in flight: which session, and which verb, so each button
 * can tell "I am running" from "something else is". */
type SessionAction = {
  path: string;
  verb: "dismiss" | "restore" | "delete";
};

const ACTION_LABEL: Record<SessionAction["verb"], string> = {
  dismiss: "Dismiss",
  restore: "Restore",
  delete: "Delete",
};

/**
 * Meetings that were captured but never became a note, each with a one-click
 * retry. The founding doc's "never silently misfile" principle extends to never
 * silently dropping: a distill failure used to surface only as a caption that
 * faded on the next capture, leaving a meeting note that simply didn't exist.
 *
 * The list is derived from disk (sessions with no note), so it survives a
 * restart and needs no failure record of its own. A silent capture is a benign
 * skip and never appears here, and a session still being distilled is excluded
 * by the backend until its run finishes. Dismissal is the one persisted bit: a
 * marker file next to the session (cleared by Restore, by a successful retry,
 * or by Delete), so a waved-off capture stays waved off across refreshes and
 * restarts while its recording stays on disk. The sidebar counts only the
 * undismissed, and this view reads the same listing, so the two can never
 * disagree.
 *
 * The view is the narrowest and airiest in the app: a short centred column of
 * pre-lifted cards, each with the marigold edge that belongs to failure and to
 * nothing else. A list of things that went wrong should feel finite and
 * finishable, not like a wall — and when it empties, an idle kodama says so
 * rather than leaving a blank page.
 */
export function NeedsAttentionView() {
  const { sessions, loading, error } = useFailedSessions();
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  // One dismiss/restore/delete in flight at a time. These are fast filesystem
  // writes; serializing them keeps the disabled/loading story as simple as
  // Retry's.
  const [actionPending, setActionPending] = useState<SessionAction | null>(null);
  const [showDismissed, setShowDismissed] = useState(false);
  // The dismissed row whose delete confirmation is open. Delete is the one
  // destructive verb on this screen, so it always goes through the same
  // confirmation modal the project delete uses (`DestructiveConfirmDialog`),
  // never firing on the row's first click.
  const [confirmingDeletePath, setConfirmingDeletePath] = useState<string | null>(null);
  // The error from a failed delete, shown inside the modal rather than under
  // the row. Cleared when the modal opens, closes, or its row is pruned away.
  const [deleteError, setDeleteError] = useState<string | null>(null);

  useTauriEvent<DistillEvent>(DISTILL_STATE_EVENT, (payload) => {
    if (payload.status === "distilling" || payload.status === "routing_fallback") {
      return;
    }
    // Only the run this row is waiting on may clear its pending state. Every
    // terminal event names its session, so another session finishing (an
    // automatic distill landing mid-retry) can no longer re-arm Retry for a run
    // that is still going, which would queue a second run and a second note.
    setPendingPath((current) => (current === payload.session_path ? null : current));
    if (payload.status === "error") {
      setRowErrors((current) => ({
        ...current,
        [payload.session_path]: `Retry failed: ${payload.message}`,
      }));
    }
    // No notifyVaultChanged() here. The sidebar's needs-attention row is
    // mounted for the whole session and owns that refetch, so a failure
    // surfaces wherever the user happens to be standing; doing it here too
    // would just refetch twice whenever this view is the one on screen.
  });

  // Drop messages for sessions that are no longer listed (retried successfully,
  // deleted, or pruned by the retention sweep). Without this the map only ever
  // grows, and a message from an old failure could resurface under a row that
  // came back. Pruned during render when the list identity changes (React's
  // adjust-state-on-prop-change pattern) so the drop lands before paint; the
  // length check skips the setState when nothing was pruned. A pending delete
  // confirm is dropped the same way: if the refetch removed its row, the
  // question is moot, and it must not reattach if the path ever lists again.
  const [previousSessions, setPreviousSessions] = useState(sessions);
  if (previousSessions !== sessions) {
    setPreviousSessions(sessions);
    const listed = new Set(sessions.map((session) => session.path));
    const remaining = Object.entries(rowErrors).filter(([path]) => listed.has(path));
    if (remaining.length !== Object.keys(rowErrors).length) {
      setRowErrors(Object.fromEntries(remaining));
    }
    if (confirmingDeletePath !== null && !listed.has(confirmingDeletePath)) {
      setConfirmingDeletePath(null);
      setDeleteError(null);
    }
  }

  const retry = (path: string) => {
    setPendingPath(path);
    setRowErrors((current) => {
      const next = { ...current };
      delete next[path];
      return next;
    });
    retryDistill(path).catch((err: unknown) => {
      setPendingPath(null);
      setRowErrors((current) => ({ ...current, [path]: `Retry failed: ${String(err)}` }));
    });
  };

  // Dismiss, restore, and delete share one shape: mark the row busy, run the
  // command, and on success prompt the shared refetch — the row moves (or
  // vanishes) when the new listing lands, so both this view and the sidebar
  // change together. On failure the row stays and says why.
  const runAction = (verb: SessionAction["verb"], path: string, call: (path: string) => Promise<void>) => {
    setActionPending({ path, verb });
    setRowErrors((current) => {
      const next = { ...current };
      delete next[path];
      return next;
    });
    call(path)
      .then(() => notifyVaultChanged())
      .catch((err: unknown) => {
        setRowErrors((current) => ({
          ...current,
          [path]: `${ACTION_LABEL[verb]} failed: ${String(err)}`,
        }));
      })
      .finally(() => setActionPending(null));
  };

  const isRunning = (verb: SessionAction["verb"], path: string) =>
    actionPending !== null && actionPending.verb === verb && actionPending.path === path;

  // The delete the confirmation modal commits. It routes its error into the
  // modal (`deleteError`) rather than the row's `rowErrors`, and closes the
  // modal itself on success before the shared refetch lands. No pendingPath /
  // actionPending guard is needed here the way the row buttons need one: the
  // modal's Overlay scrim blocks every other row action while it is open, so
  // nothing else can be in flight at confirm time. `actionPending` is still set
  // so the confirm button reads busy and the screen stays consistent.
  const confirmDelete = () => {
    const path = confirmingDeletePath;
    if (path === null) return;
    setActionPending({ path, verb: "delete" });
    setDeleteError(null);
    deleteSession(path)
      .then(() => {
        setConfirmingDeletePath(null);
        notifyVaultChanged();
      })
      .catch((err: unknown) => {
        setDeleteError(`Couldn't delete the capture: ${String(err)}`);
      })
      .finally(() => setActionPending(null));
  };

  const activeSessions = sessions.filter((session) => !session.dismissed);
  const dismissedSessions = sessions.filter((session) => session.dismissed);
  const confirmingSession = confirmingDeletePath
    ? dismissedSessions.find((session) => session.path === confirmingDeletePath) ?? null
    : null;
  const failedOn = lastFailure(activeSessions);

  return (
    <ViewFrame
      variant="health"
      eyebrow="System"
      title="Needs attention"
      summary={
        activeSessions.length > 0
          ? `${activeSessions.length} ${activeSessions.length === 1 ? "capture" : "captures"} to retry${
              failedOn ? ` · last failed ${failedOn}` : ""
            }`
          : undefined
      }
    >
      {error ? (
        <StatusMessage variant="error">
          Couldn&apos;t list captured sessions: {error}
        </StatusMessage>
      ) : (
        <>
          {activeSessions.length === 0 ? (
            // The sidebar row that leads here disappears at zero, so a user
            // standing on this view when the last retry succeeds would
            // otherwise watch the screen empty out with nothing to tell them
            // it went well. Say so. (The dismissed shelf below, when there is
            // one, still renders — dismissed captures are cleared, not gone.)
            !loading && (
              // An idle kodama, one line of what happened, one of what it
              // means (docs/DESIGN_SYSTEM.md §3): empty is first-run copy, not
              // an apology. The mark is aria-hidden, so the two lines carry it.
              <div className="flex flex-col items-center gap-1.5 pt-24 pb-[70px] text-center">
                {/* Held back from full ink: the idle mark is dormant, and at
                    26px a solid disc outweighs the sentence it is there to
                    accompany. Opacity rather than a colour, so it lands on the
                    faint step in both grounds without the primitive growing a
                    mode only this screen would use. */}
                <SpiritMark mode="idle" size="26px" className="mb-[18px] opacity-60" />
                <p className="text-[15px] font-semibold text-ink">All clear.</p>
                {/* Dim, not faint: `ink-faint` is the metadata register and is
                    spent on timestamps, counts and ids (docs/DESIGN_SYSTEM.md
                    §6). This is the only thing on the screen and a sentence
                    the user reads for meaning, which is the one thing that
                    register may not carry — and it is the register every other
                    view's empty line already reads at. */}
                <p className="max-w-[44ch] text-[12.5px] leading-[1.55] text-ink-dim">
                  Failed captures land here so they are never invisible. Nothing
                  needs a look.
                </p>
              </div>
            )
          ) : (
            <ul className="mt-[26px] flex flex-col gap-3.5" data-testid="needs-attention">
              {activeSessions.map((session) => (
                <li key={session.path} className={`${ATTENTION_CARD_CLASSES} border-l-warn`}>
                  <CaptureSummary session={session} rowError={rowErrors[session.path]} />
                  {/* Two actions, ranked by shape rather than by colour: Retry
                      is the one you want, so it is the action rectangle;
                      Dismiss recedes to the quiet ghost under it. The rail is
                      a column behind a hairline, so the two read as this
                      card's options rather than as page controls that happen
                      to sit nearby. */}
                  <div className="flex flex-none flex-col items-stretch gap-1.5 border-l border-edge pl-6">
                    <Button
                      data-testid="retry-distill"
                      // One retry at a time: each run spends a real headless
                      // Claude call, and the backend serializes them anyway. The
                      // running row is excluded from the `disabled` half on
                      // purpose — `loading` makes it busy-but-focusable, and a
                      // native `disabled` would blur the very control the user
                      // just pressed.
                      disabled={
                        (pendingPath !== null && pendingPath !== session.path) ||
                        actionPending !== null
                      }
                      loading={pendingPath === session.path}
                      loadingLabel="Retrying…"
                      onClick={() => retry(session.path)}
                    >
                      Retry
                    </Button>
                    <Button
                      variant="quiet"
                      // Disabled while *any* retry is in flight, not just this
                      // row's own: once a retry is queued the backend's
                      // in-flight filter drops that row from the listing, so
                      // the refetch a dismiss triggers — whichever row it came
                      // from — would remove the running row out from under
                      // `pendingPath` and leave every other Retry disabled
                      // with nothing on screen to say why. (Retry is already
                      // disabled while a marker action runs; this is the same
                      // exclusion in the other direction.)
                      disabled={
                        pendingPath !== null ||
                        (actionPending !== null && !isRunning("dismiss", session.path))
                      }
                      // `loading` with no `loadingLabel`: the marker write is
                      // a near-instant filesystem op, done before "Dismissing…"
                      // would even be readable — swapping the label just to
                      // swap it back a frame later read as a glitch, not
                      // feedback. `loading` alone still buys the inert,
                      // still-focusable treatment while it's in flight.
                      loading={isRunning("dismiss", session.path)}
                      onClick={() => runAction("dismiss", session.path, dismissSession)}
                    >
                      {/* "Dismiss", not "Discard". The backend writes a marker
                          file next to the session — the row stops counting but
                          the recording and transcript are untouched, and the
                          shelf below is where they come back from. "Discard"
                          named a deletion this button does not perform; that
                          verb now exists only behind that shelf and its
                          confirm, which is the one place on this screen that
                          spells out what is and isn't destroyed. */}
                      Dismiss
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          {dismissedSessions.length > 0 && (
            // The shelf takes the footnote's lead-in, not the list's: it is a
            // register below the live cards, related but no longer asking.
            <div className="mt-[22px]">
              {/* The shelf of waved-off captures, collapsed by default: they
                  asked to stop being counted, so they don't get to keep a seat
                  on the page — but they stay one interaction away, because a
                  dismissal you can't find again is just a slower delete. */}
              <Button
                variant="quiet"
                aria-expanded={showDismissed}
                data-testid="show-dismissed"
                onClick={() => setShowDismissed((open) => !open)}
              >
                Dismissed
                <span className="text-ink-faint"> · {dismissedSessions.length}</span>
              </Button>
              {showDismissed && (
                <ul className="mt-4 flex flex-col gap-3.5" data-testid="dismissed-sessions">
                  {dismissedSessions.map((session) => (
                    // Faint ink where a live card is marigold: marigold means
                    // failure, and a dismissed capture has stopped claiming to
                    // be one. Faint at full strength, not an alpha step below
                    // it — the muting is the title's job and the dropped
                    // badge's; an edge too dim to see just reads as a card
                    // that lost its edge.
                    <li key={session.path} className={`${ATTENTION_CARD_CLASSES} border-l-ink-faint`}>
                      <CaptureSummary
                        session={session}
                        rowError={rowErrors[session.path]}
                        dismissed
                      />
                      <div className="flex flex-none flex-col items-stretch gap-1.5 border-l border-edge pl-6">
                        <Button
                          data-testid="restore-session"
                          // `pendingPath` for the same reason as Dismiss:
                          // a restore's refetch would drop the retrying
                          // row from the listing mid-run.
                          disabled={
                            pendingPath !== null ||
                            (actionPending !== null && !isRunning("restore", session.path))
                          }
                          // Same reasoning as Dismiss: no `loadingLabel`,
                          // since the swap would resolve before it could
                          // be read.
                          loading={isRunning("restore", session.path)}
                          onClick={() => runAction("restore", session.path, restoreSession)}
                        >
                          Restore
                        </Button>
                        <Button
                          variant="quiet"
                          data-testid="delete-session"
                          // Disabled while any retry or other marker action is
                          // in flight, the same as Restore: this click opens
                          // the confirmation modal, and confirming it would fire
                          // a refetch that drops the retrying row mid-run. Once
                          // the modal is open its scrim blocks new actions, so
                          // this guard is what keeps the confirm safe.
                          disabled={
                            pendingPath !== null ||
                            (actionPending !== null && !isRunning("delete", session.path))
                          }
                          loading={isRunning("delete", session.path)}
                          onClick={() => {
                            setDeleteError(null);
                            setConfirmingDeletePath(session.path);
                          }}
                        >
                          Delete
                        </Button>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
          {(activeSessions.length > 0 || dismissedSessions.length > 0) && (
            /* One line, and it has to be true: `useCommands` keeps the jump to
               this view in the palette even when every capture is dismissed
               and the sidebar row is gone, precisely so a waved-off capture is
               never unreachable. It sits its own lead-in below the cards
               rather than tucked under the last one — it is about all of
               them.

               Inherited `font-ui`, not `font-data`: this is a sentence in the
               interface's own voice, and the data face carries the things that
               line up in a column — clocks, counts, ids, paths, eyebrows
               (docs/DESIGN_SYSTEM.md §1). */
            <p className="mt-[22px] text-[11px] text-ink-dim">
              Dismissed captures stay reachable from the command palette.
            </p>
          )}
          {confirmingSession && (
            <DestructiveConfirmDialog
              title="Delete this capture?"
              subject={sessionTitle(confirmingSession)}
              confirmLabel="Delete capture"
              busyLabel="Deleting…"
              busy={isRunning("delete", confirmingSession.path)}
              error={deleteError}
              onConfirm={confirmDelete}
              onClose={() => {
                setConfirmingDeletePath(null);
                setDeleteError(null);
              }}
            >
              <p>
                Its recording and transcript are deleted. Dismiss only hides a
                capture, and deletes nothing.
              </p>
            </DestructiveConfirmDialog>
          )}
        </>
      )}
    </ViewFrame>
  );
}
