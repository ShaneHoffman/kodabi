import { useState } from "react";
import { captureLabel, markMode } from "../../captureLabel";
import { isCaptureActive, useCaptureState } from "../../useCaptureState";
import { PALETTE_SHORTCUT_LABEL } from "../../useCommandPalette";
import { useDebouncedValue } from "../../useDebouncedValue";
import { useElapsed } from "../../useElapsed";
import { useNavigation } from "../../useNavigation";
import { ListenPill } from "./ListenPill";

/** The two chrome links, which are the same control with different content.
 * Not the Button primitive: those are rectangles that carry weight, and these
 * are the quietest thing in the window — chrome you reach for, not actions the
 * view is about. */
const TOPLINK_CLASS =
  "focus-ring inline-flex items-center gap-2 rounded-[8px] px-2.5 py-1.5 text-[12.5px] " +
  "text-ink-dim transition-colors duration-140 ease-out-strong hover:bg-wash hover:text-ink";

type Props = {
  onOpenPalette: () => void;
};

/**
 * The window's transport bar: what the app is called, what capture is doing,
 * and the two pieces of chrome that belong to the window rather than to any
 * view.
 *
 * It exists so the listening indicator can be the one thing in the app that
 * never moves. In the old sidebar foot it sat below a list that grows, and it
 * shared a rail with destinations — so an on-air surface competed with places
 * to go. Here it sits beside the wordmark, above everything, at a fixed
 * height: whatever screen you are on, the answer to "is it recording" is in
 * the same place.
 *
 * Commands and Settings live here for the same reason. They are not
 * destinations in the knowledge base, and mixing them into the dock made the
 * dock a list of two different kinds of thing.
 */
export function TopBar({ onOpenPalette }: Props) {
  const { view, navigate } = useNavigation();
  const captureState = useCaptureState();

  // The mark reacts instantly for immediate visual feedback; the label — an
  // aria-live region inside the pill — follows a debounced state so a flapping
  // VAD doesn't spam screen readers on every toggle. (The contract the sidebar
  // foot's indicator used to hold.)
  const label = captureLabel(useDebouncedValue(captureState, 400));
  const mode = markMode(captureState);

  // Timed from the moment the session engaged, not from the first audio: the
  // backend reports a phase, not a duration. Held during render rather than in
  // an effect — it is derived from a value changing, which is the
  // adjust-state-during-render pattern (QuickCapture.tsx does the same).
  const engaged = isCaptureActive(captureState.phase);
  const [engagedSince, setEngagedSince] = useState<number | null>(null);
  const [wasEngaged, setWasEngaged] = useState(engaged);
  if (wasEngaged !== engaged) {
    setWasEngaged(engaged);
    setEngagedSince(engaged ? Date.now() : null);
  }
  const elapsedSeconds = useElapsed(engagedSince);

  return (
    <header className="glass-top flex h-[54px] flex-none items-center gap-6 px-[22px]">
      {/* The document's h1: heading navigation needs a level-1 root. It reads
          quietly by design (preflight strips h1 sizing) and doubles as the way
          home, which is the one thing a wordmark in a window is for. */}
      <h1 className="text-[15px] font-semibold tracking-[0.02em]">
        <button
          type="button"
          className="focus-ring rounded-[6px]"
          onClick={() => navigate({ kind: "inbox" })}
        >
          kodabi
        </button>
      </h1>

      {/* The detail travels with the label, not just the label: a start whose
          every source failed derives phase `idle`, and no other surface says
          so — the tray reads "Kodabi: idle", the start notification is
          suppressed for a start that captured nothing, the overlay pill
          renders nothing while capture is inactive, and CaptureToast only
          carries distill and transcription failures. */}
      <ListenPill
        mode={mode}
        label={label.text}
        detail={label.detail}
        elapsedSeconds={elapsedSeconds}
      />

      <nav aria-label="App" className="ml-auto flex items-center gap-1.5">
        <button
          type="button"
          aria-haspopup="dialog"
          onClick={onOpenPalette}
          className={TOPLINK_CLASS}
        >
          Commands
          {/* A real <kbd>: it is a key the user presses, which is exactly what
              the element means. Faint because it is metadata about the control
              beside it, not a second label. */}
          <kbd className="rounded-[4px] border border-edge px-1.5 py-0.5 font-data text-[11px] font-normal text-ink-faint">
            {PALETTE_SHORTCUT_LABEL}
          </kbd>
        </button>
        <button
          type="button"
          aria-current={view.kind === "settings" ? "page" : undefined}
          onClick={() => navigate({ kind: "settings" })}
          className={TOPLINK_CLASS}
        >
          Settings
        </button>
      </nav>
    </header>
  );
}
