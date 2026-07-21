import { useCommandPalette } from "../useCommandPalette";
import { useConsentNudge } from "../useConsentNudge";
import { useNavigation } from "../useNavigation";
import { useSessionsChangedBridge } from "../useSessionsChangedBridge";
import { useVaultChangedBridge } from "../useVaultChangedBridge";
import { AppErrorBoundary } from "./AppErrorBoundary";
import { CaptureToast } from "./CaptureToast";
import { CommandPalette } from "./CommandPalette";
import { ConsentNudge } from "./ConsentNudge";
import { MainContent } from "./MainContent";
import { Sidebar } from "./Sidebar";

/**
 * The persistent layout every destination docks into: sidebar + main region,
 * with the command palette overlaid on demand. The shell owns the viewport
 * (h-screen); sidebar and main scroll independently.
 */
export function AppShell() {
  const { open, openPalette, closePalette } = useCommandPalette();
  const { open: consentOpen, closeNudge } = useConsentNudge();
  const { view } = useNavigation();
  // Refresh this window's lists when another window (quick capture) writes.
  useVaultChangedBridge();
  // ...and when the retention sweep prunes the raw sessions behind them.
  useSessionsChangedBridge();

  return (
    <div className="flex h-screen overflow-hidden bg-bg font-sans text-text">
      <Sidebar onOpenPalette={openPalette} />
      <main className="flex-1 overflow-y-auto">
        {/* Only the routed view is guarded: a crash here leaves the sidebar
            alive, so the user navigates out rather than restarting. */}
        <AppErrorBoundary resetKey={view.kind}>
          <MainContent />
        </AppErrorBoundary>
      </main>
      {/* Outside the error boundary and outside the routed view: the
          pipeline keeps running whatever screen you are on, and its outcome
          has to reach you there. */}
      <CaptureToast />
      {open && <CommandPalette onClose={closePalette} />}
      {consentOpen && <ConsentNudge onClose={closeNudge} />}
    </div>
  );
}
