import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  hideQuickCaptureWindow,
  submitQuickCapture,
} from "../quickCapture";
import "./QuickCapture.css";

/** The window came to the foreground (backend `show_window`): refocus + reset. */
const SHOWN_EVENT = "quick-capture:shown";

/** How long the destination flashes before the window dismisses itself. Short
 * enough to still feel instant, long enough to read where the note landed. */
const FLASH_MS = 600;

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

  // Re-show resets to a clean, focused box. The draft in `text` is deliberately
  // left intact (an Escape'd thought survives the next pop); only a successful
  // submit clears it.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen(SHOWN_EVENT, () => {
      if (!active) return;
      setStatus({ kind: "idle" });
      textareaRef.current?.focus();
    }).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

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
    setStatus({ kind: "submitting" });
    submitQuickCapture(trimmed)
      .then((outcome) => {
        setText("");
        setStatus({ kind: "filed", destination: outcome.project ?? "Inbox" });
      })
      .catch((err: unknown) => {
        // Stay open with the draft intact so the thought isn't lost.
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
