import { useCaptureState } from "./useCaptureState";
import { useDebouncedValue } from "./useDebouncedValue";
import { useTranscriptionState } from "./useTranscriptionState";
import { SpiritMark } from "./components/SpiritMark";

function transcriptionLabel(state: ReturnType<typeof useTranscriptionState>): string | null {
  switch (state.status) {
    case "transcribing":
      return "Transcribing…";
    case "saved":
      return "Saved";
    case "error":
      return "Transcription failed";
    case "idle":
      return null;
  }
}

function App() {
  const phase = useCaptureState();
  // The mark reacts instantly for immediate visual feedback, but the text
  // label — an aria-live region — follows a debounced phase so a flapping VAD
  // doesn't spam screen readers (or flicker the label) on every toggle.
  const settled = useDebouncedValue(phase, 400) === "listening";
  const transcription = useTranscriptionState(phase);
  const transcriptionText = transcriptionLabel(transcription);

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
      {transcriptionText && (
        <p role="status" className="text-cap uppercase tracking-wide text-text-soft">
          {transcriptionText}
        </p>
      )}
    </main>
  );
}

export default App;
