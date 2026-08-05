import { MotionConfig } from "motion/react";
import { useCommandPalette } from "../../useCommandPalette";
import { useConsentNudge } from "../../useConsentNudge";
import { useNavigation, viewKey } from "../../useNavigation";
import { useReduceMotion } from "../../useReduceMotion";
import { useSessionsChangedBridge } from "../../useSessionsChangedBridge";
import { useVaultChangedBridge } from "../../useVaultChangedBridge";
import { AppErrorBoundary } from "./AppErrorBoundary";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { CaptureToast } from "../overlays/CaptureToast";
import { CommandPalette } from "../overlays/CommandPalette";
import { ConsentNudge } from "../overlays/ConsentNudge";
import { Dock } from "./Dock";
import { MainContent } from "./MainContent";
import { TopBar } from "./TopBar";

/**
 * The persistent layout every destination docks into: a transport bar over a
 * dock and a main panel, with the command palette overlaid on demand.
 *
 * Three glass surfaces on the grove ground, and the gutter between them is the
 * point — the dock and the panel are panes floating on a lit background, not
 * regions of one sheet, so each carries its own edges and its own scroll. The
 * shell owns the viewport (h-screen) and nothing else scrolls.
 */
export function AppShell() {
  const { open, openPalette, closePalette } = useCommandPalette();
  const { open: consentOpen, closeNudge } = useConsentNudge();
  const { view } = useNavigation();
  const reduceMotion = useReduceMotion();
  // Refresh this window's lists when another window (quick capture) writes.
  useVaultChangedBridge();
  // ...and when the retention sweep prunes the raw sessions behind them.
  useSessionsChangedBridge();

  return (
    // The one capture/transcription/distill subscription, above the error
    // boundary so a crashed routed view doesn't tear it down, and around both
    // the Inbox (which shows the pipeline's progress) and the toast (which
    // now only ever shows its failures).
    <CapturePipelineProvider>
      {/* The reduced-motion policy for everything the `motion` package drives,
          set once here so a call site cannot forget it. "always" strips the
          transforms and layout animations and keeps the opacity ones, which is
          the app's rule exactly: movement is the accessibility problem, life is
          not (docs/DESIGN_SYSTEM.md §4). It is fed by our own hook rather than
          motion's `reducedMotion="user"` alone, because that only reads the OS
          query and would ignore the app's own switch. */}
      <MotionConfig reducedMotion={reduceMotion ? "always" : "user"}>
        <div className="grove-ground flex h-screen flex-col overflow-hidden font-ui text-ink">
          <TopBar onOpenPalette={openPalette} />
          <div className="flex min-h-0 flex-1 gap-5 p-5">
            <Dock />
            <main className="glass-panel min-w-0 flex-1 overflow-y-auto">
              {/* Only the routed view is guarded: a crash here leaves the dock
                  and the transport bar alive, so the user navigates out rather
                  than restarting. The key is the whole destination, not just its
                  kind — the fallback tells the user to pick another screen, and
                  picking a second project (or a second note, or a second search)
                  has to actually clear it. */}
              <AppErrorBoundary resetKey={viewKey(view)}>
                <MainContent />
              </AppErrorBoundary>
            </main>
          </div>
          {/* Outside the error boundary and outside the routed view: a failure
              reaches you whatever screen you are on. */}
          <CaptureToast />
          {/* Mounted whether or not it is open, unlike the nudge below: the
              palette owns its own exit animation, and base-ui only keeps a popup
              alive through that animation if the element is still rendered.
              Behind `open &&` the dissolve would be cut off by the unmount. */}
          <CommandPalette open={open} onClose={closePalette} />
          {consentOpen && <ConsentNudge onClose={closeNudge} />}
        </div>
      </MotionConfig>
    </CapturePipelineProvider>
  );
}
