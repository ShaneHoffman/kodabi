import { useCaptureState } from "./useCaptureState";
import { useDebouncedValue } from "./useDebouncedValue";
import { SpiritMark } from "./components/SpiritMark";

function App() {
  const phase = useCaptureState();
  // The mark reacts instantly for immediate visual feedback, but the text
  // label — an aria-live region — follows a debounced phase so a flapping VAD
  // doesn't spam screen readers (or flicker the label) on every toggle.
  const settled = useDebouncedValue(phase, 400) === "listening";

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-md bg-bg text-text font-sans">
      <SpiritMark phase={phase} size="2.5rem" halo="2.4rem" />
      <h1 className="font-serif text-display tracking-tight">kodabi</h1>
      <p
        role="status"
        className={`text-cap uppercase tracking-wide ${
          settled ? "text-accent-dot" : "text-text-soft"
        }`}
      >
        {settled ? "Listening" : "Idle"}
      </p>
    </main>
  );
}

export default App;
