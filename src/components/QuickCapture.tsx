import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { hideQuickCaptureWindow, submitQuickCapture } from "../quickCapture";
import { useTauriEvent } from "../useTauriEvent";
import { QUICK_CAPTURE_SHOWN_EVENT } from "../events";
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
 * The quick-capture window's whole UI: one text box. Enter files the thought
 * through the routing pipeline, flashes where it landed, then the window
 * dismisses; Escape (or losing focus, handled backend-side) hides it with the
 * draft preserved. Window show/hide lives in Rust — this component only submits
 * text and asks the backend to hide.
 */
export function QuickCapture() {
  const [text, setText] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  // A monotonic "capture session" counter, bumped every time the box comes
  // forward. Captured when a submit starts; an in-flight submit that resolves
  // after the box was re-shown (a new session) is stale and must not touch the
  // UI — else its `setText("")` / flash-and-hide clobbers the fresh draft the
  // user has since started (or its late error wipes a capture they've moved on
  // to). See the guards in `submit`.
  const sessionRef = useRef(0);

  // Re-show refocuses the box. A prior *error* keeps its message and draft so a
  // blur-dismiss can't silently bury a failed capture — the user sees it on the
  // next pop. Any other prior status (a stale success flash, a leftover
  // "submitting") resets to a clean idle box. The draft in `text` is otherwise
  // left intact (an Escape'd thought survives the next pop); only a successful
  // submit clears it.
  useTauriEvent(QUICK_CAPTURE_SHOWN_EVENT, () => {
    sessionRef.current += 1;
    setStatus((prev) => (prev.kind === "error" ? prev : { kind: "idle" }));
    textareaRef.current?.focus();
  });

  // Flash the destination, then dismiss. The timer is the "then hide" half of
  // the submit; cleared on unmount or if status moves on (e.g. a re-show).
  useEffect(() => {
    if (status.kind !== "filed") return;
    const timer = setTimeout(() => void hideQuickCaptureWindow(), FLASH_MS);
    return () => clearTimeout(timer);
  }, [status]);

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

  const onKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    // Keys mid-IME-composition belong to the composition, not the box.
    if (event.nativeEvent.isComposing) return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      void hideQuickCaptureWindow();
    }
  };

  return (
    <main className="quick-capture__panel flex h-screen flex-col gap-2xs bg-surface p-sm">
      <textarea
        ref={textareaRef}
        autoFocus
        spellCheck={false}
        placeholder="Capture a thought…"
        value={text}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={onKeyDown}
        aria-label="Capture a thought"
        className="quick-capture__input min-h-0 flex-1 resize-none rounded-md bg-bg-sink px-xs py-2xs text-body text-text placeholder:text-text-faint"
      />
      <footer className="flex items-baseline justify-between gap-2xs text-cap text-text-faint">
        <span className={status.kind === "error" ? "text-text-soft" : undefined}>
          {status.kind === "error"
            ? status.message
            : "Enter files it · Shift+Enter for a new line · Esc dismisses"}
        </span>
        {status.kind === "submitting" && <span className="text-text-soft">Filing…</span>}
        {status.kind === "filed" && (
          <span className="text-text">→ {status.destination}</span>
        )}
      </footer>
    </main>
  );
}
