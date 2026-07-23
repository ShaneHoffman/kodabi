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
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./NeedsAttentionView.css";

/** A readable name for a session: its slug de-hyphenated, else an honest
 * placeholder. Never the capture time, which the line below already shows, and
 * never the filename — a session captured without a slug is named
 * `20260719T190540729Z-5fonvqzd.jsonl`, and setting a machine id in the reading
 * serif was the single ugliest thing on the screen. */
function sessionTitle(session: FailedSession): string {
  const slug = session.slug?.split("-").filter(Boolean).join(" ").trim();
  return slug || "Untitled capture";
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
 * An outline circle with a bang. INK, never red — this palette has no red,
 * and rank is never carried by hue (docs/DESIGN.md). A failed capture is not
 * an emergency; it is a thing to redo, and drawing it in alarm colours would
 * be the interface panicking on the user's behalf.
 */
function WarningGlyph() {
  return (
    <svg
      width="19"
      height="19"
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
      className="block flex-none text-text-faint"
    >
      <circle cx="10" cy="10" r="8.2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M10 5.6v5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      <circle cx="10" cy="13.8" r="0.95" fill="currentColor" />
    </svg>
  );
}

/** The title-and-caption column both card kinds share. A dismissed card mutes
 * its title: still readable, no longer asking for anything. */
function CaptureSummary({
  session,
  rowError,
  muted,
}: {
  session: FailedSession;
  rowError?: string;
  muted?: boolean;
}) {
  return (
    <div className="min-w-0 flex-1">
      <p className={`text-lead font-semibold ${muted ? "text-text-soft" : "text-text"}`}>
        {sessionTitle(session)}
      </p>
      <p className="mt-3xs font-mono text-cap text-text-faint">
        {formatCaptureTime(session.captured_at)} · Kodabi made no note from it
      </p>
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
 * pre-lifted cards. A list of things that went wrong should feel finite and
 * finishable, not like a wall.
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
  // The dismissed row whose Delete is waiting on its inline confirm. Delete is
  // the one destructive verb on this screen, so it never fires on the first
  // click — but a modal would be heavier than the act deserves.
  const [confirmingDeletePath, setConfirmingDeletePath] = useState<string | null>(null);

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

  const activeSessions = sessions.filter((session) => !session.dismissed);
  const dismissedSessions = sessions.filter((session) => session.dismissed);
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
              <StatusMessage variant="empty">
                All clear. Captures that never became a note land here.
              </StatusMessage>
            )
          ) : (
            <ul className="attention__stack" data-testid="needs-attention">
              {activeSessions.map((session) => (
                <li key={session.path} className="attention__card">
                  <WarningGlyph />
                  <CaptureSummary session={session} rowError={rowErrors[session.path]} />
                  {/* Two actions, ranked by weight rather than by colour: Retry
                      is the one you want, so it carries full ink and semibold;
                      Dismiss recedes to muted regular. */}
                  <div className="flex flex-none items-center gap-sm">
                    <Button
                      variant="quiet"
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
                      className="py-3xs text-label font-semibold text-text"
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
                      className="py-3xs text-label text-text-soft"
                    >
                      {/* "Dismiss", not "Discard". The backend writes a marker
                          file next to the session — the row stops counting but
                          the recording and transcript are untouched, which is
                          exactly what the footnote promises. "Discard" named a
                          deletion this button does not perform; that verb now
                          exists only behind the dismissed shelf and its
                          confirm. */}
                      Dismiss
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          {dismissedSessions.length > 0 && (
            <div className="attention__dismissed">
              {/* The shelf of waved-off captures, collapsed by default: they
                  asked to stop being counted, so they don't get to keep a seat
                  on the page — but they stay one interaction away, because a
                  dismissal you can't find again is just a slower delete. */}
              <Button
                variant="quiet"
                aria-expanded={showDismissed}
                data-testid="show-dismissed"
                className="font-mono text-meta text-text-soft"
                onClick={() => setShowDismissed((open) => !open)}
              >
                Dismissed
                <span className="text-text-faint"> · {dismissedSessions.length}</span>
              </Button>
              {showDismissed && (
                <ul className="attention__dismissed-stack" data-testid="dismissed-sessions">
                  {dismissedSessions.map((session) => (
                    <li key={session.path} className="attention__card">
                      <CaptureSummary
                        session={session}
                        rowError={rowErrors[session.path]}
                        muted
                      />
                      <div className="flex flex-none items-center gap-sm">
                        {confirmingDeletePath === session.path ? (
                          <>
                            {/* The inline confirm: same spot, second click.
                                Restore and Delete swap out so the question is
                                the only thing to answer, and Cancel restores
                                them unchanged. */}
                            <span className="text-cap text-text-soft">Delete for good?</span>
                            <Button
                              variant="quiet"
                              data-testid="confirm-delete-session"
                              // `pendingPath` for the same reason as Dismiss:
                              // the refetch a delete triggers would drop the
                              // retrying row from the listing mid-run.
                              disabled={pendingPath !== null || actionPending !== null}
                              onClick={() => {
                                setConfirmingDeletePath(null);
                                runAction("delete", session.path, deleteSession);
                              }}
                              className="py-3xs text-label font-semibold text-text"
                            >
                              Delete
                            </Button>
                            <Button
                              variant="quiet"
                              data-testid="cancel-delete-session"
                              onClick={() => setConfirmingDeletePath(null)}
                              className="py-3xs text-label text-text-soft"
                            >
                              Cancel
                            </Button>
                          </>
                        ) : (
                          <>
                            <Button
                              variant="quiet"
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
                              className="py-3xs text-label font-semibold text-text"
                            >
                              Restore
                            </Button>
                            <Button
                              variant="quiet"
                              data-testid="delete-session"
                              // `pendingPath` too, even though this click only
                              // opens the confirm: arming a delete whose
                              // confirm is disabled would be a dead end.
                              disabled={
                                pendingPath !== null ||
                                (actionPending !== null && !isRunning("delete", session.path))
                              }
                              // Same reasoning as Dismiss.
                              loading={isRunning("delete", session.path)}
                              onClick={() => setConfirmingDeletePath(session.path)}
                              className="py-3xs text-label text-text-soft"
                            >
                              Delete
                            </Button>
                          </>
                        )}
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
          {(activeSessions.length > 0 || dismissedSessions.length > 0) && (
            /* The reassurance, and it has to match what the code implements:
               dismissal is a marker the backend persists and Restore clears,
               so "until you restore it" is a promise that survives refreshes
               and restarts. Delete is the one destructive verb, and it lives
               only behind the dismissed shelf and its confirm — the footnote
               says so plainly rather than pretending the screen deletes
               nothing at all. */
            <p className="attention__footnote text-cap text-text-faint">
              Retrying re-runs distillation on the original recording.
              Dismissing hides a capture until you restore it, and deletes
              nothing. Deleting from the dismissed list removes the recording
              and transcript for good.
            </p>
          )}
        </>
      )}
    </ViewFrame>
  );
}
