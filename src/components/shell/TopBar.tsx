import { useState, type ReactNode } from "react";
import { CAPTURE_TOGGLE_SHORTCUT } from "../../captureControl";
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

/** Every control in the bar's right cluster — the two chrome links and the
 * three caption buttons alike. Not the Button primitive: those are rectangles
 * that carry weight, and these are the quietest thing in the window, chrome you
 * reach for rather than actions the view is about.
 *
 * Icon-only, and square: the bar is the window's title bar now, so every word
 * in it is width the wordmark and the listening pill do not get. The names
 * survive as `aria-label` (unchanged for screen readers) plus `title`, which is
 * where the palette's shortcut went too.
 *
 * One shape for both kinds, deliberately. The caption buttons started as
 * full-height flat targets in the Windows idiom, flush to the top and right
 * edges so Close landed in the screen corner when maximized; sitting beside two
 * inset rounded squares, that made five controls in a row wearing two different
 * uniforms, and the seam read as an accident rather than a category. The corner
 * is the price, and the divider does the separating instead. */
const CHROME_BUTTON_BASE =
  "focus-ring inline-flex size-9 flex-none items-center justify-center rounded-[8px] " +
  "text-ink-dim transition-colors duration-140 ease-out-strong";

const CHROME_BUTTON_CLASS = `${CHROME_BUTTON_BASE} hover:bg-wash hover:text-ink`;

/** Close, which is the one control here that hover treats differently: red, as
 * every Windows title bar has taught every Windows user to expect. The tokens
 * are the destructive-confirmation pair, so it lands as Grove's red rather than
 * the OS hex, and it flips with the ground like everything else.
 *
 * It reads as a stronger claim than the button makes — closing hides to the
 * tray (`lib.rs` prevents the close and hides), so nothing is destroyed, and
 * DESIGN_SYSTEM.md §2 otherwise keeps red for the confirm control inside a
 * confirmation. The convention wins anyway: a title bar's rightmost button is
 * read by muscle memory long before anyone reads its colour, and being the one
 * button that answers differently is what makes it findable at a glance. */
const CLOSE_BUTTON_CLASS = `${CHROME_BUTTON_BASE} hover:bg-danger-bg-hover hover:text-danger`;

/** The chrome marks: 16px, one hairline weight up from the caption glyphs
 * below, matching the Select chevron's 1.25. Same reason those are drawn rather
 * than typed — this app carries no icon font and no icon package, and two marks
 * do not justify a seventh dependency (`.claude/rules/typescript-style.md`). */
function ChromeIcon({ children }: { children: ReactNode }) {
  return (
    <svg
      className="size-4"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.25"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

/** The four caption glyphs, drawn rather than typed. Drawn as a set — a font's
 * × would drift from its stroked neighbours in weight and baseline, however
 * carefully it were sized.
 *
 * 12px on a 10-unit grid, so the 1-wide stroke lands at 1.2 on screen: a
 * hair under the 1.25 the chrome marks are drawn at, which is as close to one
 * weight as two grids get. Bare glyph shapes read larger than a detailed mark
 * at the same size, so they stay a step smaller than their 16px neighbours to
 * end up looking the same size. */
function CaptionGlyph({ children }: { children: ReactNode }) {
  return (
    <svg
      className="size-[12px]"
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

  // The one place in the main window that teaches the capture chord, and the
  // reason it is here rather than on a first-run banner: it sits on the very
  // indicator the chord operates, so the lesson and the thing it is about are
  // the same object. Permanent on every idle pill by design, the way an editor
  // keeps its shortcut in the placeholder rather than retiring it once — the
  // pill hides its detail while live, so the hint is gone exactly when it stops
  // being true.
  const shortcutHint = `${CAPTURE_TOGGLE_SHORTCUT} starts a capture`;

  const maximized = useWindowMaximized();

  return (
    // The bare drag attribute, not `deep` (CaptureOverlayPill is the contrast):
    // it hit-tests this element alone, so the bar's own background drags the
    // window while every child — wordmark, pill, links, caption buttons —
    // keeps its own pointer behaviour. Double-click-to-maximize comes with it.
    //
    // Less padding on the right than the left, and the two edges still look
    // even: a 16px mark centred in a 36px button carries 10px of its own inset,
    // so 12px here puts the × the same 22px from the window edge that the
    // wordmark sits from it on the other side.
    <header
      data-tauri-drag-region
      className="glass-top flex h-[54px] flex-none items-center gap-6 pr-3 pl-[22px]"
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
        //
        // The chord hint is the last rung, so the chain reads failure, then
        // models, then teaching: something wrong always outranks something to
        // learn. That ordering is also what keeps first run right — while the
        // models download, the more urgent fact holds the slot, and the hint
        // takes over the moment transcription is ready.
        detail={label.detail ?? modelsDetail ?? shortcutHint}
        elapsedSeconds={elapsedSeconds}
      />

      {/* One cluster, two groups, a rule between them: the shape no longer says
          which controls belong to the app and which to the window, so the rule
          has to. It is the whole separation, which is why the gap either side of
          it is tighter than the header's own. */}
      <div className="ml-auto flex items-center gap-2">
        <nav aria-label="App" className="flex items-center gap-1.5">
          {/* The shortcut is part of the control's own name rather than a <kbd>
              beside it: a screen reader hears the key without hunting for a
              second node, and a mouse user gets it from the tooltip. That is
              what makes dropping the visible keycap a tightening, not a loss. */}
          <button
            type="button"
            aria-haspopup="dialog"
            aria-label={`Commands (${PALETTE_SHORTCUT_LABEL})`}
            title={`Commands (${PALETTE_SHORTCUT_LABEL})`}
            onClick={onOpenPalette}
            className={CHROME_BUTTON_CLASS}
          >
            {/* The overflow ellipsis, unframed: this is the window's drawer of
                everything you can do, which is what those three dots mean in a
                Windows title bar. The frame it started with was the problem,
                not the dots — every enclosing shape this app could draw is
                already spoken for. A magnifier is the dock's Search, a prompt
                caret its Terminal, dots inside a bubble its Chat, a rule across
                a sheet is a bank card, and a stack of rules is the sliders next
                door. */}
            <ChromeIcon>
              {/* Filled, and a touch fatter than a hairline outline would be:
                  three 1.25-wide rings at this size are three grey smudges, and
                  the mark has to carry the same weight as the sliders beside it
                  or it reads as disabled. */}
              <circle cx="3.9" cy="8" r="1.1" fill="currentColor" stroke="none" />
              <circle cx="8" cy="8" r="1.1" fill="currentColor" stroke="none" />
              <circle cx="12.1" cy="8" r="1.1" fill="currentColor" stroke="none" />
            </ChromeIcon>
          </button>
          <button
            type="button"
            aria-current={view.kind === "settings" ? "page" : undefined}
            aria-label="Settings"
            title="Settings"
            onClick={() => navigate({ kind: "settings" })}
            className={CHROME_BUTTON_CLASS}
          >
            {/* Sliders, not a gear: a gear's teeth turn to mush at 16px under a
                hairline stroke, and this is the same two-rail mark the settings
                screen is actually made of. Each rail breaks either side of its
                knob rather than running under it — two hairlines crossing at
                this size read as a smudge, not as a control on a track. */}
            <ChromeIcon>
              <path d="M2.5 5.5h1.75M7.75 5.5h5.75" />
              <circle cx="6" cy="5.5" r="1.75" />
              <path d="M2.5 10.5h5.75M11.75 10.5h1.75" />
              <circle cx="10" cy="10.5" r="1.75" />
            </ChromeIcon>
          </button>
        </nav>

        {/* Short, not full-height: it separates two rows of controls, so it is
            drawn to their height and not to the bar's. Decorative, hence no
            role and nothing for a screen reader to stop on — the `nav` landmark
            already tells that story to anyone not looking at it. */}
        <span aria-hidden="true" className="h-4 w-px flex-none bg-edge" />

        {/* Outside the nav: these belong to the window, not to the app. */}
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            aria-label="Minimize"
            title="Minimize"
            onClick={minimizeWindow}
            className={CHROME_BUTTON_CLASS}
          >
            <CaptionGlyph>
              <path d="M0.5 5h9" />
            </CaptionGlyph>
          </button>
          <button
            type="button"
            aria-label={maximized ? "Restore" : "Maximize"}
            title={maximized ? "Restore" : "Maximize"}
            onClick={toggleMaximizeWindow}
            className={CHROME_BUTTON_CLASS}
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
          {/* Known loss on the button above: hovering a *native* maximize
              button opens the Windows 11 Snap Layouts flyout, and an
              undecorated window cannot offer it — that flyout is native caption
              hit-testing. Accepted for v1; Win+Z, Win+arrow and the edge-drag
              snaps all still work. */}
          <button
            type="button"
            aria-label="Close"
            title="Close"
            onClick={closeWindow}
            className={CLOSE_BUTTON_CLASS}
          >
            <CaptionGlyph>
              <path d="M1 1l8 8M9 1l-8 8" />
            </CaptionGlyph>
          </button>
        </div>
      </div>
    </header>
  );
}
