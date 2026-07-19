import { useEffect, useState } from "react";
import { DISTILL_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { retryDistill, type FailedSession } from "../../useSessions";
import { notifyVaultChanged } from "../../useVaultQuery";
import type { DistillEvent } from "../../useDistillState";
import { Button } from "../ui/Button";

/** A readable name for a session: its slug de-hyphenated, else the raw filename.
 * Never the capture time, which the line below the title already shows. */
function sessionTitle(session: FailedSession): string {
  const slug = session.slug?.split("-").filter(Boolean).join(" ").trim();
  return slug || session.file_name;
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
  // back. Returning `current` unchanged when nothing was pruned keeps React
  // from re-rendering on every refetch.
  useEffect(() => {
    setRowErrors((current) => {
      const listed = new Set(sessions.map((session) => session.path));
      const remaining = Object.entries(current).filter(([path]) =>
        listed.has(path),
      );
      return remaining.length === Object.keys(current).length
        ? current
        : Object.fromEntries(remaining);
    });
  }, [sessions]);

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

  return (
    <section className="flex flex-col gap-3xs" data-testid="needs-attention">
      <p className="text-eyebrow uppercase tracking-wide text-text-faint">
        Needs attention
      </p>
      {error ? (
        <p className="text-cap text-text-soft">{error}</p>
      ) : (
        <ul className="flex flex-col gap-3xs">
          {sessions.map((session) => (
            <li
              key={session.path}
              className="flex items-start justify-between gap-md py-2xs"
            >
              <div className="flex min-w-0 flex-1 flex-col gap-3xs">
                <span className="font-serif text-body text-text-soft">
                  {sessionTitle(session)}
                </span>
                <span className="text-cap text-text-faint">
                  {formatCaptureTime(session.captured_at)} · no note was created
                </span>
                {rowErrors[session.path] && (
                  <p className="text-cap text-text-soft">
                    {rowErrors[session.path]}
                  </p>
                )}
              </div>
              <div className="w-48 flex-none text-right">
                {pendingPath === session.path ? (
                  <span className="text-cap text-text-faint">Retrying…</span>
                ) : (
                  <Button
                    variant="quiet"
                    data-testid="retry-distill"
                    // One retry at a time: each run spends a real headless
                    // Claude call, and the backend serializes them anyway.
                    disabled={pendingPath !== null}
                    onClick={() => retry(session.path)}
                    className="text-body text-text-soft hover:text-text"
                  >
                    Retry
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
