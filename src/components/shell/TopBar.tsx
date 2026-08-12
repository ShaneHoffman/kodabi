import { useState, type ReactNode } from "react";
import { captureLabel, markMode } from "../../captureLabel";
import { isCaptureActive, useCaptureState } from "../../useCaptureState";
import { PALETTE_SHORTCUT_LABEL } from "../../useCommandPalette";
import { useDebouncedValue } from "../../useDebouncedValue";
import { useElapsed } from "../../useElapsed";
import { useModelStatus, isTranscriptionReady } from "../../useModelStatus";
import { useNavigation } from "../../useNavigation";
import { useWindowMaximized } from "../../useWindowMaximized";
import { closeWindow, minimizeWindow, toggleMaximizeWindow } from "../../windowControls";
import { ListenPill } from "./ListenPill";

/** The two chrome links, which are the same control with different content.
 * Not the Button primitive: those are rectangles that carry weight, and these
 * are the quietest thing in the window — chrome you reach for, not actions the
 * view is about. */
const TOPLINK_CLASS =
  "focus-ring inline-flex items-center gap-2 rounded-[8px] px-2.5 py-1.5 text-[12.5px] " +
  "text-ink-dim transition-colors duration-140 ease-out-strong hover:bg-wash hover:text-ink";

/** The window's caption buttons. Quieter cousins of the chrome links above and
 * for the same reason, but a different shape: full-height flat targets in the
 * Windows caption idiom, flush to the top and right edges so the close button
 * sits in the screen corner when the window is maximized. TOPLINK's inset
 * rounded pill cannot reach a corner. `focus-ring-inset` because an outward
 * ring would be clipped by the window edge. */
const CAPTION_CLASS =
  "focus-ring-inset inline-flex h-full w-[46px] flex-none items-center justify-center " +
  "text-ink-dim transition-colors duration-140 ease-out-strong hover:bg-wash hover:text-ink";

/** The four caption glyphs, drawn rather than typed. A hairline stroke, one
 * step under the Select chevron's 1.25: these are the quietest marks in the
 * window. Drawn as a set — a font's × would drift from its stroked neighbours
 * in weight and baseline, however carefully it were sized. */
function CaptionGlyph({ children }: { children: ReactNode }) {
  return (
    <svg
      className="size-[10px]"
      viewBox="0 0 10 10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

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

  // Transcription is downstream of capture, so its readiness is never a claim
  // about whether audio is reaching disk — which is why this only ever fills
  // the detail slot and never touches the mark or the headline.
  const { state: models } = useModelStatus();
  const modelsDetail = isTranscriptionReady(models) ? null : "Transcription not ready yet";

  const maximized = useWindowMaximized();

  return (
    // The bare drag attribute, not `deep` (CaptureOverlayPill is the contrast):
    // it hit-tests this element alone, so the bar's own background drags the
    // window while every child — wordmark, pill, links, caption buttons —
    // keeps its own pointer behaviour. Double-click-to-maximize comes with it.
    //
    // No right padding: the caption buttons run to the window edge, so that
    // Close is in the screen corner when maximized.
    <header
      data-tauri-drag-region
      className="glass-top flex h-[54px] flex-none items-center gap-6 pl-[22px]"
    >
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
        // A capture failure always outranks it: that is about the recording
        // itself, this is about what happens afterwards. The models line is a
        // fallback rather than an addition because the pill shows one detail,
        // and it only reaches an idle pill anyway — the detail slot is hidden
        // while live, which is exactly right here. Nothing is wrong during the
        // recording; it is the transcription afterwards that is waiting.
        detail={label.detail ?? modelsDetail}
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

      {/* Outside the nav: these belong to the window, not to the app — the
          header's gap is the whole separation, since the shape change from
          rounded pill to full-height flat target already reads as a different
          category of control. */}
      <div className="flex h-full items-stretch">
        <button
          type="button"
          aria-label="Minimize"
          onClick={minimizeWindow}
          className={CAPTION_CLASS}
        >
          <CaptionGlyph>
            <path d="M0.5 5h9" />
          </CaptionGlyph>
        </button>
        <button
          type="button"
          aria-label={maximized ? "Restore" : "Maximize"}
          onClick={toggleMaximizeWindow}
          className={CAPTION_CLASS}
        >
          {maximized ? (
            <CaptionGlyph>
              {/* The back pane, showing as an arc behind the front one. */}
              <path d="M3 0.5h4.5a2 2 0 0 1 2 2V7" />
              <rect x="0.5" y="2.5" width="7" height="7" rx="1.5" />
            </CaptionGlyph>
          ) : (
            <CaptionGlyph>
              <rect x="0.5" y="0.5" width="9" height="9" rx="1.5" />
            </CaptionGlyph>
          )}
        </button>
        {/* Close hides to the tray (`lib.rs` prevents the close and hides), so
            it gets the same quiet hover as its neighbours rather than the
            Windows red: nothing here is destroyed, and DESIGN_SYSTEM.md keeps
            danger for the confirm control inside a confirmation.

            Known loss: hovering a *native* maximize button opens the Windows 11
            Snap Layouts flyout, and an undecorated window cannot offer it —
            that flyout is native caption hit-testing. Accepted for v1; Win+Z,
            Win+arrow and the edge-drag snaps all still work. */}
        <button type="button" aria-label="Close" onClick={closeWindow} className={CAPTION_CLASS}>
          <CaptionGlyph>
            <path d="M1 1l8 8M9 1l-8 8" />
          </CaptionGlyph>
        </button>
      </div>
    </header>
  );
}
