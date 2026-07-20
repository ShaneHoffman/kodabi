import { useState } from "react";
import { DISTILL_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { retryDistill, type FailedSession } from "../../useSessions";
import { notifyVaultChanged } from "../../useVaultQuery";
import type { DistillEvent } from "../../useDistillState";
import { Button } from "../ui/Button";
import { ListRow } from "../ui/ListRow";
import { StatusMessage } from "../ui/StatusMessage";

/** A readable name for a session: its slug de-hyphenated, else an honest
 * placeholder. Never the capture time, which the line below already shows, and
 * never the filename — a session captured without a slug is named
 * `20260719T190540729Z-5fonvqzd.jsonl`, and setting a machine id in the reading
 * serif was the single ugliest thing on the screen. */
function sessionTitle(session: FailedSession): string {
  const slug = session.slug?.split("-").filter(Boolean).join(" ").trim();
  return slug || "Untitled capture";
}

/** How many rows show before the section collapses behind an expander. This
 * section is an exception list living inside a view named for something else;
 * unbounded, it measured 498px against the Inbox's own 199px and pushed the
 * notes it sits above off the screen entirely (docs/DESIGN_SYSTEM.md §1). */
const COLLAPSED_ROWS = 3;

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
 * restart and needs no failure record of its own. It renders nothing at all
 * when there is nothing to act on, which is the normal case: a silent capture is
 * a benign skip and never appears here, and a session still being distilled is
 * excluded by the backend until its run finishes.
 *
 * The list itself is owned by `InboxView`, which needs to know whether there is
 * anything here before deciding its own "nothing waiting" empty state.
 */
export function NeedsAttentionSection({
  sessions,
  error,
}: {
  sessions: FailedSession[];
  error: string | null;
}) {
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const [expanded, setExpanded] = useState(false);

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
      // A distill that just failed on its own (not a retry from here) belongs in
      // this list right away, while the Inbox is open to see it.
      notifyVaultChanged();
    }
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

  // Nothing waiting and nothing broken: stay out of the way entirely.
  if (!error && sessions.length === 0) return null;

  const hidden = Math.max(0, sessions.length - COLLAPSED_ROWS);
  const visible = expanded ? sessions : sessions.slice(0, COLLAPSED_ROWS);

  return (
    <section className="flex flex-col gap-3xs" data-testid="needs-attention">
      <p className="text-eyebrow uppercase tracking-eyebrow text-text-faint">
        Needs attention
      </p>
      {error ? (
        <StatusMessage variant="error" compact>
          Couldn&apos;t list captured sessions: {error}
        </StatusMessage>
      ) : (
        <>
          <ul className="flex flex-col gap-3xs">
            {visible.map((session) => (
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
          {hidden > 0 && (
            <Button
              variant="quiet"
              onClick={() => setExpanded(!expanded)}
              // -ml-xs cancels the primitive's own control padding so the
              // label's text edge lines up with the row titles above it,
              // rather than sitting 12px proud of the column.
              className="-ml-xs self-start text-cap text-text-faint"
            >
              {expanded ? "Show fewer" : `Show ${hidden} more`}
            </Button>
          )}
        </>
      )}
    </section>
  );
}
