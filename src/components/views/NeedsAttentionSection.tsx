import { useState } from "react";
import { DISTILL_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { retryDistill, useFailedSessions, type FailedSession } from "../../useSessions";
import { notifyVaultChanged } from "../../useVaultQuery";
import type { DistillState } from "../../useDistillState";
import { Button } from "../ui/Button";

/** The wire payload of `distill:state`, including the non-terminal warning this
 * section ignores. Mirrors `useDistillState`'s `DistillEvent`. */
type DistillEvent = DistillState | { status: "routing_fallback"; message: string };

/** A readable name for a session: its slug de-hyphenated, else the capture day. */
function sessionTitle(session: FailedSession): string {
  const slug = session.slug?.split("-").filter(Boolean).join(" ").trim();
  if (slug) return slug;
  return `Captured ${formatCaptureTime(session.captured_at)}`;
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
 */
export function NeedsAttentionSection() {
  const { sessions, error } = useFailedSessions();
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});

  useTauriEvent<DistillEvent>(DISTILL_STATE_EVENT, (payload) => {
    if (payload.status === "saved" || payload.status === "skipped") {
      // The run finished; a successful distill also broadcasts `vault:changed`,
      // which refetches this list and drops the row.
      setPendingPath(null);
      return;
    }
    if (payload.status === "error") {
      setPendingPath((current) =>
        current === payload.session_path ? null : current,
      );
      setRowErrors((current) => ({
        ...current,
        [payload.session_path]: payload.message,
      }));
      // A distill that just failed on its own (not a retry from here) belongs in
      // this list right away, while the Inbox is open to see it.
      notifyVaultChanged();
    }
  });

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
