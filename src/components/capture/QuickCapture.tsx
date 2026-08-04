import {
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { captureLabel } from "../../captureLabel";
import { startCapture, stopCapture } from "../../captureControl";
import { hideQuickCaptureWindow, submitQuickCapture } from "../../quickCapture";
import { isCaptureActive, useCaptureState } from "../../useCaptureState";
import { useDebouncedValue } from "../../useDebouncedValue";
import { formatElapsed, useElapsed } from "../../useElapsed";
import { useTauriEvent } from "../../useTauriEvent";
import { useTimeout } from "../../useTimeout";
import { QUICK_CAPTURE_SHOWN_EVENT } from "../../events";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; the capture windows' Grove ticket deletes it
import "./QuickCapture.css";

/** How long the destination flashes before the window dismisses itself. Short
 * enough to still feel instant, long enough to read where the note landed.
 * Exported so the test asserts against this value rather than a copy of it —
 * a copy only catches the constant growing, never it shrinking to nothing. */
export const FLASH_MS = 600;

type Status =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "filed"; destination: string }
  | { kind: "error"; message: string };

/**
 * The quick-capture window: a thought, typed or spoken, in one summoned box.
 *
 * It reads in three registers. TYPING is silent — serif text, a green caret,
 * and nothing else moving. ENGAGED BUT NOT ON AIR (starting up, or every
 * source dropped and reconnecting) wears the full recording chrome — header,
 * timer, Stop — in ink: the session is running and Enter must stop it, but
 * nothing is reaching disk so nothing may claim the green. SPEAKING turns the
 * green fully live: a breathing dot, four animated bars, and a timer. That is
 * the whole reason the hue is reserved; this window is where it pays off,
 * because "is this thing recording me" has to be answerable from the doorway.
 *
 * The two questions are kept apart deliberately. `isCaptureActive` decides
 * which chrome is mounted, what Enter and Escape do, and when the clock runs;
 * `captureLabel().live` decides the green, and nothing else.
 *
 * Window show/hide lives in Rust — this component submits text, starts and
 * stops the capture, and asks the backend to hide.
 */
export function QuickCapture() {
  const [text, setText] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [captureError, setCaptureError] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // A monotonic "capture session" counter, bumped every time the box comes
  // forward. Captured when a submit starts; an in-flight submit that resolves
  // after the box was re-shown (a new session) is stale and must not touch the
  // UI — else its `setText("")` / flash-and-hide clobbers the fresh draft the
  // user has since started (or its late error wipes a capture they've moved on
  // to). See the guards in `submit`.
  const sessionRef = useRef(0);

  const captureState = useCaptureState();
  const engaged = isCaptureActive(captureState.phase);

  // Timed from the moment the session engaged — the press — not from the
  // moment audio started arriving, so a source dropping out and recovering
  // mid-capture never rewinds the clock to 0:00 and under-reports how long
  // this recording has been going. The backend reports a phase, not a
  // duration, so this is the honest source — accurate to within a tick, which
  // is the resolution a `0:07` shows anyway. Held during render rather than in
  // an effect: it is derived from a prop-like value changing.
  const [engagedSince, setEngagedSince] = useState<number | null>(null);
  const [wasEngaged, setWasEngaged] = useState(engaged);
  if (wasEngaged !== engaged) {
    setWasEngaged(engaged);
    setEngagedSince(engaged ? Date.now() : null);
  }
  const elapsed = useElapsed(engagedSince);

  // Re-show refocuses the box. A prior *error* keeps its message and draft so a
  // blur-dismiss can't silently bury a failed capture — the user sees it on the
  // next pop. Any other prior status (a stale success flash, a leftover
  // "submitting") resets to a clean idle box. The draft in `text` is otherwise
  // left intact (an Escape'd thought survives the next pop); only a successful
  // submit clears it.
  useTauriEvent(QUICK_CAPTURE_SHOWN_EVENT, () => {
    sessionRef.current += 1;
    setStatus((prev) => (prev.kind === "error" ? prev : { kind: "idle" }));
    inputRef.current?.focus();
  });

  // Flash the destination, then dismiss. The timer is the "then hide" half of
  // the submit; cleared on unmount or if status moves on (e.g. a re-show).
  useTimeout(() => void hideQuickCaptureWindow(), status.kind === "filed" ? FLASH_MS : null);

  const submit = () => {
    const trimmed = text.trim();
    if (!trimmed || status.kind === "submitting") return; // guards Enter-repeat
    const session = sessionRef.current;
    setStatus({ kind: "submitting" });
    submitQuickCapture(trimmed)
      .then((outcome) => {
        // Re-shown since submit: the note still landed (and `vault:changed`
        // already refreshed the main window) — just don't clear the new draft
        // or dismiss the box out from under the user.
        if (sessionRef.current !== session) return;
        setText("");
        setStatus({ kind: "filed", destination: outcome.project ?? "Inbox" });
      })
      .catch((err: unknown) => {
        // Same guard: if the user already moved on to a new capture, don't
        // clobber it with a stale failure. Otherwise stay open with the draft
        // and error intact — preserved across a hide/show so a blur-dismiss
        // can't lose the thought.
        if (sessionRef.current !== session) return;
        setStatus({ kind: "error", message: String(err) });
      });
  };

  // Enter means "finish what I am doing", and what that is depends on the
  // state: it stops a recording, or it files a typed thought.
  //
  // A draft typed alongside a recording is filed too. The box invites one
  // ("Add a note alongside the recording…") and it is a separate note, not part
  // of the transcript, so nothing downstream would ever pick it up — leaving it
  // in the box means losing it when the window hides.
  const stopAndFile = () => {
    setCaptureError(null);
    stopCapture().catch((err: unknown) => setCaptureError(String(err)));
    if (text.trim()) submit();
  };

  const record = () => {
    setCaptureError(null);
    startCapture().catch((err: unknown) => setCaptureError(String(err)));
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    // Keys mid-IME-composition belong to the composition, not the box.
    if (event.nativeEvent.isComposing) return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      if (engaged) stopAndFile();
      else submit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (engaged) stopAndFile();
      void hideQuickCaptureWindow();
    }
  };

  return (
    <main className="capture flex h-screen flex-col">
      <div className="capture__body flex-1">
        {engaged ? <Listening elapsed={elapsed} state={captureState} /> : null}
        {/* The box stays mounted while recording rather than being replaced:
            a spoken capture and a typed one land in the same note, so what you
            have already typed must not vanish the moment you press Record. */}
        <div className={engaged ? "mt-sm" : ""}>
          <textarea
            ref={inputRef}
            data-testid="quick-capture-input"
            aria-label="Capture a thought"
            autoFocus
            spellCheck={false}
            rows={engaged ? 2 : 3}
            placeholder={engaged ? "Add a note alongside the recording…" : "Capture a thought…"}
            value={text}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={onKeyDown}
            className="capture__input ui-focus-ring ui-writing placeholder:text-text-faint"
          />
        </div>
      </div>

      {/* A real <hr>: a thematic break between the thought and the transport
          under it, which is what the element means. */}
      <hr className="capture__rule" />

      <footer className="capture__footer">
        {status.kind === "error" ? (
          // role="alert": a failed capture arrives asynchronously and the user
          // may not be looking at the box.
          <StatusMessage variant="error" compact>
            Couldn&apos;t file this: {status.message}
          </StatusMessage>
        ) : captureError ? (
          <StatusMessage variant="error" compact>
            Couldn&apos;t reach the recorder: {captureError}
          </StatusMessage>
        ) : (
          <span className="font-mono text-micro text-text-faint">
            {engaged
              ? // Not "Esc cancels". There is no cancel: Escape runs the same
                // stop-and-file path Enter does, and a hint that promised
                // otherwise would be a lie about a recording.
                "↵ or Esc stops and files"
              : "↵ files it · ⇧↵ new line · Esc dismisses"}
          </span>
        )}

        {status.kind === "filed" ? (
          <span
            data-testid="quick-capture-destination"
            className="flex-none font-mono text-micro text-text"
          >
            → {status.destination}
          </span>
        ) : engaged ? (
          <button
            type="button"
            onClick={stopAndFile}
            className="capture__stop ui-focus-ring flex-none text-action font-semibold text-text"
          >
            <span className="capture__square" aria-hidden="true" />
            Stop
          </button>
        ) : (
          // Two affordances, ranked by weight: Record is a quiet ghost because
          // it starts something, File it is the action rectangle because it
          // ends something. Both visible, because this window is opened by
          // hotkey but must not be operable by hotkey alone
          // (docs/DESIGN_SYSTEM.md §6).
          <div className="flex flex-none items-center gap-sm">
            <Button variant="quiet" data-testid="quick-capture-record" onClick={record}>
              <span className="capture__ring" aria-hidden="true" />
              Record
            </Button>
            <Button
              data-testid="quick-capture-submit"
              onClick={submit}
              disabled={!text.trim()}
              loading={status.kind === "submitting"}
              loadingLabel="Filing…"
            >
              File it
            </Button>
          </div>
        )}
      </footer>
    </main>
  );
}

/**
 * The recording header: mounted for the whole session, green only when live.
 *
 * A dot inside a breathing glow, four bars, the state as a word, and a timer.
 * Together they answer the only question that matters here without being read:
 * something is being recorded, and it has been for this long.
 *
 * It follows the same contract every on-air surface does, for the same reason:
 * the state reads through the dot's FILL and the label's VALUE, never through
 * a change of shape. A capture that is starting up, or whose sources have all
 * dropped, keeps every part of this header in place — same dot, same bars,
 * same timer — but wears them in ink, because the green means precisely one
 * thing and nothing is reaching disk yet. The label says which it is.
 */
function Listening({
  elapsed,
  state,
}: {
  elapsed: number;
  state: ReturnType<typeof useCaptureState>;
}) {
  // The mark reacts instantly for immediate visual feedback, but the label —
  // a live region — follows a debounced state so a flapping source doesn't
  // spam screen readers (or flicker the word) on every toggle.
  const label = captureLabel(useDebouncedValue(state, 400));
  const live = captureLabel(state).live;
  return (
    <div className="flex items-center gap-xs">
      <span className="capture__dot">
        {live && <span className="capture__glow" aria-hidden="true" />}
        <span className={`capture__core${live ? " capture__core--live" : ""}`} />
      </span>
      <p
        role="status"
        className={`text-label font-semibold ${live ? "text-text" : "text-text-faint"}`}
      >
        {label.text}
      </p>
      <span
        className={`capture__wave${live ? "" : " capture__wave--quiet"}`}
        aria-hidden="true"
      >
        <span className="capture__bar" />
        <span className="capture__bar" />
        <span className="capture__bar" />
        <span className="capture__bar" />
      </span>
      <span className="ml-auto font-mono text-cap text-text-faint">
        {formatElapsed(elapsed)}
      </span>
    </div>
  );
}
