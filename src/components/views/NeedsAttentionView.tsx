import { useState } from "react";
import { DISTILL_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { retryDistill, useFailedSessions, type FailedSession } from "../../useSessions";
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
 * The view is the narrowest and airiest in the app: a short centred column of
 * pre-lifted cards. A list of things that went wrong should feel finite and
 * finishable, not like a wall.
 */
export function NeedsAttentionView() {
  const { sessions, loading, error } = useFailedSessions();
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  // Cards the user has waved off this session. Discard is deliberately NOT a
  // delete: the recording is untouched on disk and the card returns on the
  // next listing. It clears the flag, not the data — which is exactly what the
  // footnote promises, and why it needs no confirmation.
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());

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
    const remaining = Object.entries(rowErrors).filter(([path]) => listed.has(path));
    if (remaining.length !== Object.keys(rowErrors).length) {
      setRowErrors(Object.fromEntries(remaining));
    }
    // Discards last until the next listing, and no longer. Held any longer
    // they would make this view claim "All clear" while the sidebar row beside
    // it still counted the same sessions, which is the one thing a
    // needs-attention surface must never do.
    if (dismissed.size > 0) setDismissed(new Set());
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

  const visible = sessions.filter((session) => !dismissed.has(session.path));
  const failedOn = lastFailure(visible);

  return (
    <ViewFrame
      variant="health"
      eyebrow="System"
      title="Needs attention"
      summary={
        visible.length > 0
          ? `${visible.length} ${visible.length === 1 ? "capture" : "captures"} to retry${
              failedOn ? ` · last failed ${failedOn}` : ""
            }`
          : undefined
      }
    >
      {error ? (
        <StatusMessage variant="error">
          Couldn&apos;t list captured sessions: {error}
        </StatusMessage>
      ) : visible.length === 0 ? (
        // The sidebar row that leads here disappears at zero, so a user standing
        // on this view when the last retry succeeds would otherwise watch the
        // screen empty out with nothing to tell them it went well. Say so.
        !loading && (
          <StatusMessage variant="empty">
            All clear. Captures that never became a note land here.
          </StatusMessage>
        )
      ) : (
        <>
          <ul className="attention__stack" data-testid="needs-attention">
            {visible.map((session) => (
              <li key={session.path} className="attention__card">
                <WarningGlyph />
                <div className="min-w-0 flex-1">
                  <p className="text-lead font-semibold text-text">
                    {sessionTitle(session)}
                  </p>
                  <p className="mt-3xs font-mono text-cap text-text-faint">
                    {formatCaptureTime(session.captured_at)} · no note was created
                  </p>
                  {rowErrors[session.path] && (
                    <StatusMessage variant="error" compact>
                      Retry failed: {rowErrors[session.path]}
                    </StatusMessage>
                  )}
                </div>
                {/* Two actions, ranked by weight rather than by colour: Retry
                    is the one you want, so it carries full ink and semibold;
                    Discard recedes to muted regular. */}
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
                    disabled={pendingPath !== null && pendingPath !== session.path}
                    loading={pendingPath === session.path}
                    loadingLabel="Retrying…"
                    onClick={() => retry(session.path)}
                    className="py-3xs text-label font-semibold text-text"
                  >
                    Retry
                  </Button>
                  <Button
                    variant="quiet"
                    // Disabled while this row's own retry is in flight. The row
                    // owns `pendingPath`, so discarding it used to strand that
                    // value and leave every other Retry disabled with nothing
                    // on screen to say why.
                    disabled={pendingPath === session.path}
                    onClick={() =>
                      setDismissed((current) =>
                        new Set(current).add(session.path),
                      )
                    }
                    className="py-3xs text-label text-text-faint"
                  >
                    Discard
                  </Button>
                </div>
              </li>
            ))}
          </ul>
          {/* The one piece of reassurance on the screen, and it belongs here
              rather than in a tooltip: the whole reason Discard needs no
              confirmation is that nothing it touches is destroyed. */}
          <p className="attention__footnote text-cap text-text-faint">
            Retrying re-runs distillation on the original recording. Nothing was
            deleted.
          </p>
        </>
      )}
    </ViewFrame>
  );
}
