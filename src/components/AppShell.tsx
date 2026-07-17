import { useCommandPalette } from "../useCommandPalette";
import { CommandPalette } from "./CommandPalette";
import { MainContent } from "./MainContent";
import { Sidebar } from "./Sidebar";

/**
 * The persistent layout every destination docks into: sidebar + main region,
 * with the command palette overlaid on demand. The shell owns the viewport
 * (h-screen); sidebar and main scroll independently.
 */
export function AppShell() {
  const { open, openPalette, closePalette } = useCommandPalette();

  return (
    <div className="flex h-screen overflow-hidden bg-bg font-sans text-text">
      <Sidebar onOpenPalette={openPalette} />
      <main className="flex-1 overflow-y-auto">
        <MainContent />
      </main>
      {open && <CommandPalette onClose={closePalette} />}
    </div>
  );
}
