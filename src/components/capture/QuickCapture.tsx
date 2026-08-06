import { clsx } from "clsx";
import {
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { captureLabel, markMode } from "../../captureLabel";
import { startCapture, stopCapture } from "../../captureControl";
import { hideQuickCaptureWindow, submitQuickCapture } from "../../quickCapture";
import { isCaptureActive, useCaptureState } from "../../useCaptureState";
import { useDebouncedValue } from "../../useDebouncedValue";
import { formatElapsed, useEngagedElapsed } from "../../useElapsed";
import { folderHue, type FolderHue } from "../../useProjects";
import { useRoutePreview } from "../../useRoutePreview";
import { useTauriEvent } from "../../useTauriEvent";
import { useTimeout } from "../../useTimeout";
import { QUICK_CAPTURE_SHOWN_EVENT } from "../../events";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { isTranscriptionReady } from "../../useModelStatus";
import { useModelDownload } from "../../useModelDownload";
import { SpiritMark } from "./SpiritMark";

/** How long the destination flashes before the window dismisses itself. Short
 * enough to still feel instant, long enough to read where the note landed.
 * Exported so the test asserts against this value rather than a copy of it —
 * a copy only catches the constant growing, never it shrinking to nothing. */
export const FLASH_MS = 600;

/* The routing guess wears its project's own colour, dot and words together:
   the guess IS the identity of a folder, and a coloured dot beside grey text
   would say two things where the user reads one (docs/DESIGN_SYSTEM.md §6).
   Written out rather than interpolated — Tailwind cannot see a constructed
   class name and would emit neither. */
const HUE_TEXT: Record<FolderHue, string> = {
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

type Status =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "filed"; destination: string }
  | { kind: "error"; message: string };

/**
 * The quick-capture window: a thought, typed or spoken, in one summoned box.
 *
 * It reads in three registers. TYPING is silent — the kodama sits idle in ink,
 * because a typed thought records nothing; only the caret is green. ENGAGED BUT
 * NOT ON AIR (starting up, or every source dropped and reconnecting) wears the
 * full recording chrome — label, timer, Stop — still in ink: the session is
 * running and Enter must stop it, but nothing is reaching disk so nothing may
 * claim the green. SPEAKING turns the mark fully live, breathing green beside a
 * running clock. That is the whole reason the hue is reserved; this window is
 * where it pays off, because "is this thing recording me" has to be answerable
 * from the doorway.
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
  // Timed from the press, not from audio arriving, so a dropout never rewinds
  // the clock. The mark reports whether sound is reaching disk; this reports
  // how long the session has been open, and they are different questions.
  const elapsed = useEngagedElapsed(engaged);

  // Where Enter would file this draft, refreshed as it is typed.
  const routeGuess = useRoutePreview(text);

  // This window is its own webview with no providers, so it subscribes
  // directly — the same reason it calls `useCaptureState` itself.
  const { state: models } = useModelDownload();
  const modelsNotice = isTranscriptionReady(models)
    ? null
    : engaged
      ? "This recording is saved. It will be transcribed once the models are ready."
      : models.status === "downloading"
        ? "Recording works. Transcripts start when the model download finishes."
        : "Recording works. Transcripts wait for a one time model download in Settings.";

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
    // The window is larger than the sheet: it is transparent and casts its own
    // shadow in CSS, which needs room to fade into or it clips flat against the
    // window bounds and reads as a rectangle around a rounded sheet.
    <main className="flex h-screen w-screen items-center justify-center p-10">
      <div className="glass-sheet flex h-full w-full flex-col overflow-hidden p-2.5">
        <div className="flex min-h-0 flex-1 items-start gap-2.5 px-3 pt-2.5 pb-2">
          {/* One mark, always mounted: idle ink while this is only a typed
              thought, the capture's own mode once a session is running. It
              turns green when audio flows and at no other time. */}
          <SpiritMark
            mode={engaged ? markMode(captureState) : "idle"}
            size="13px"
            halo="10px"
            className="mt-1"
          />
          <div className="flex min-w-0 flex-1 flex-col gap-1.5">
            {engaged && <RecordingStatus elapsed={elapsed} state={captureState} />}
            {/* The box stays mounted while recording rather than being
                replaced: a spoken capture and a typed one land in the same
                note, so what you have already typed must not vanish the moment
                you press Record. */}
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
              className="focus-ring w-full resize-none bg-transparent p-0 font-ui text-[15px] leading-relaxed text-ink caret-kodama outline-hidden placeholder:text-ink-faint"
            />
          </div>
        </div>

        {/* A real <hr>: a thematic break between the thought and the transport
            under it, which is what the element means. Preflight strips the
            default border, so the rule is the background. */}
        <hr className="h-px flex-none border-none bg-edge" />

        <footer className="flex flex-none items-center gap-3 px-3 pt-2.5 pb-1">
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
          ) : modelsNotice ? (
            // Recording is never blocked for want of models: the audio is kept
            // and transcribed on a later launch, so refusing the capture would
            // lose a real meeting to protect the user from nothing. What must
            // not happen is the user believing a transcript is coming when it
            // is not, so the hint slot says so instead. `role="status"`, not
            // `alert`: nothing has gone wrong.
            <span
              role="status"
              data-testid="quick-capture-models-notice"
              className="min-w-0 flex-1 font-data text-[10.5px] text-ink-faint"
            >
              {modelsNotice}
            </span>
          ) : (
            <span className="min-w-0 flex-1 truncate font-data text-[10.5px] text-ink-faint">
              {engaged ? (
                // Not "Esc cancels". There is no cancel: Escape runs the same
                // stop-and-file path Enter does, and a hint that promised
                // otherwise would be a lie about a recording.
                <>
                  <span className="text-ink-dim">Enter</span> or{" "}
                  <span className="text-ink-dim">Esc</span> stops and files
                </>
              ) : (
                <>
                  <span className="text-ink-dim">Enter</span> saves and routes it
                  · <span className="text-ink-dim">Esc</span> dismisses
                </>
              )}
            </span>
          )}

          {/* The router's live guess, in the slot the filed destination will
              use. Hidden the moment anything definitive needs that slot, so a
              guess never competes with what actually happened. */}
          {status.kind === "idle" && !engaged && routeGuess && (
            <span
              data-testid="quick-capture-route-preview"
              className={clsx(
                "flex flex-none items-center gap-1.5 font-data text-[10.5px]",
                routeGuess.project
                  ? HUE_TEXT[folderHue(routeGuess.project)]
                  : "text-ink-faint",
              )}
            >
              {routeGuess.project && (
                <span
                  aria-hidden="true"
                  className={clsx(
                    "size-[7px] flex-none rounded-[2px]",
                    HUE_DOT[folderHue(routeGuess.project)],
                  )}
                />
              )}
              → {routeGuess.project ?? "Inbox"}
            </span>
          )}

          {status.kind === "filed" ? (
            <span
              data-testid="quick-capture-destination"
              className="flex-none font-data text-[11px] text-ink"
            >
              → {status.destination}
            </span>
          ) : engaged ? (
            <Button variant="action" onClick={stopAndFile} className="flex-none">
              <span
                aria-hidden="true"
                className="size-[9px] flex-none rounded-[2px] bg-ink"
              />
              Stop
            </Button>
          ) : (
            // Two affordances, ranked by weight: Record is a quiet ghost because
            // it starts something, File it is the action rectangle because it
            // ends something. Both visible, because this window is opened by
            // hotkey but must not be operable by hotkey alone
            // (docs/DESIGN_SYSTEM.md §6).
            <div className="flex flex-none items-center gap-3">
              <Button variant="quiet" data-testid="quick-capture-record" onClick={record}>
                <span
                  aria-hidden="true"
                  className="size-[11px] flex-none rounded-full border border-ink-faint"
                />
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
      </div>
    </main>
  );
}

/**
 * The recording status line: what the session is doing, and for how long.
 *
 * Mounted only while a capture is engaged, which is also what seeds its
 * debounce: mounting mid-capture reads the state it mounted with, rather than
 * spending 400ms insisting the recording is idle.
 *
 * The state reads through the mark's FILL and this label's VALUE, never through
 * a change of shape — a capture that is starting up, or whose sources have all
 * dropped, keeps the same line in place and wears it in ink, because the green
 * means precisely one thing and nothing is reaching disk yet.
 */
function RecordingStatus({
  elapsed,
  state,
}: {
  elapsed: number;
  state: ReturnType<typeof useCaptureState>;
}) {
  // The mark reacts instantly for immediate feedback, but the label — a live
  // region — follows a debounced state so a flapping source doesn't spam screen
  // readers (or flicker the word) on every toggle.
  const label = captureLabel(useDebouncedValue(state, 400));
  const live = captureLabel(state).live;

  return (
    <div className="flex items-center gap-2.5">
      {/* The live region is the label alone: wrapping the clock in it would
          announce a new time every second, forever. */}
      <p
        role="status"
        className={clsx(
          "font-ui text-[11px] font-semibold tracking-[0.12em] uppercase",
          "transition-colors duration-300 ease-out-strong",
          live ? "text-kodama-ink" : "text-ink-dim",
        )}
      >
        {label.text}
      </p>
      <span className="ml-auto flex-none font-data text-[13px] text-ink tabular-nums">
        {formatElapsed(elapsed)}
      </span>
    </div>
  );
}

