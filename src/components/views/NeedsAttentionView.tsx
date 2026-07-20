import { useState } from "react";
import { DISTILL_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { retryDistill, useFailedSessions, type FailedSession } from "../../useSessions";
import type { DistillEvent } from "../../useDistillState";
import { Button } from "../ui/Button";
import { ListRow } from "../ui/ListRow";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

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

/**
 * Meetings that were captured but never became a note, each with a one-click
 * retry. The founding doc's "never silently misfile" principle extends to never
 * silently dropping: a distill failure used to surface only as a caption that
 * faded on the next capture, leaving a meeting note that simply didn't exist.
 *
 * The list is derived from disk (sessions with no note), so it survives a
 * restart and needs no failure record of its own. A silent capture is a benign
 * skip and never appears here, and a session still being distilled is excluded
 * by the backend until its run finishes.
 *
 * This was a section inside the Inbox, capped at three rows behind an expander
 * so it couldn't push the filing queue off the screen. That cap was a mitigation
 * for the real problem: two queues with different verbs and different urgency
 * sharing one column. Here the list is the subject of its own view, so it
 * renders in full and the cap is gone.
 */
export function NeedsAttentionView() {
  const { sessions, loading, error } = useFailedSessions();
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});

  useTauriEvent<DistillEvent>(DISTILL_STATE_EVENT, (payload) => {
    if (payload.status === "distilling" || payload.status === "routing_fallback") {
      return;
    }
    // Only the run this row is waiting on may clear its pending state. Every
    // terminal event names its session, so another session finishing (an
    // automatic distill landing mid-retry) can no longer re-arm Retry for a run
    // that is still going, which would queue a second run and a second note.
    setPendingPath((current) =>
      current === payload.session_path ? null : current,
    );
    if (payload.status === "error") {
      setRowErrors((current) => ({
        ...current,
        [payload.session_path]: payload.message,
      }));
    }
    // No notifyVaultChanged() here. The sidebar's needs-attention row is
    // mounted for the whole session and owns that refetch, so a failure
    // surfaces wherever the user happens to be standing; doing it here too
    // would just refetch twice whenever this view is the one on screen.
  });

  // Drop messages for sessions that are no longer listed (retried successfully,
  // or pruned by the retention sweep). Without this the map only ever grows,
  // and a message from an old failure could resurface under a row that came
  // back. Pruned during render when the list identity changes (React's
  // adjust-state-on-prop-change pattern) so the drop lands before paint; the
  // length check skips the setState when nothing was pruned.
  const [previousSessions, setPreviousSessions] = useState(sessions);
  if (previousSessions !== sessions) {
    setPreviousSessions(sessions);
    const listed = new Set(sessions.map((session) => session.path));
    const remaining = Object.entries(rowErrors).filter(([path]) =>
      listed.has(path),
    );
    if (remaining.length !== Object.keys(rowErrors).length) {
      setRowErrors(Object.fromEntries(remaining));
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
      setRowErrors((current) => ({ ...current, [path]: String(err) }));
    });
  };

  return (
    <ViewFrame
      variant="queue"
      eyebrow="System"
      title="Needs attention"
      summary={
        sessions.length > 0
          ? `${sessions.length} ${sessions.length === 1 ? "capture" : "captures"} to retry`
          : undefined
      }
    >
      {error ? (
        <StatusMessage variant="error">
          Couldn&apos;t list captured sessions: {error}
        </StatusMessage>
      ) : sessions.length === 0 ? (
        // The sidebar row that leads here disappears at zero, so a user standing
        // on this view when the last retry succeeds would otherwise watch the
        // screen empty out with nothing to tell them it went well. Say so.
        !loading && (
          <StatusMessage variant="empty">
            All clear. Captures that never became a note land here.
          </StatusMessage>
        )
      ) : (
        <ul className="flex flex-col gap-3xs" data-testid="needs-attention">
          {sessions.map((session) => (
            <li key={session.path}>
              <ListRow
                title={sessionTitle(session)}
                meta={`${formatCaptureTime(session.captured_at)} · no note was created`}
                action={
                  // The button stays mounted and focusable while its retry
                  // runs. It used to be swapped for a <span>Retrying…</span>,
                  // which unmounted the focused control and dropped focus to
                  // <body> (docs/DESIGN_SYSTEM.md §6).
                  <div className="text-right">
                    <Button
                      variant="quiet"
                      data-testid="retry-distill"
                      // One retry at a time: each run spends a real headless
                      // Claude call, and the backend serializes them anyway.
                      // The running row is excluded from the `disabled` half
                      // on purpose — `loading` makes it busy-but-focusable,
                      // and a native `disabled` would blur the very control
                      // the user just pressed.
                      disabled={pendingPath !== null && pendingPath !== session.path}
                      loading={pendingPath === session.path}
                      loadingLabel="Retrying…"
                      onClick={() => retry(session.path)}
                      className="text-body text-text-soft"
                    >
                      Retry
                    </Button>
                  </div>
                }
              />
              {rowErrors[session.path] && (
                <StatusMessage variant="error" compact>
                  Retry failed: {rowErrors[session.path]}
                </StatusMessage>
              )}
            </li>
          ))}
        </ul>
      )}
    </ViewFrame>
  );
}
