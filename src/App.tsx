import { useCaptureState } from "./useCaptureState";
import { SpiritMark } from "./components/SpiritMark";

function App() {
  const phase = useCaptureState();
  const listening = phase === "listening";

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-md bg-bg text-text font-sans">
      <SpiritMark phase={phase} size="2.5rem" halo="2.4rem" />
      <h1 className="font-serif text-display tracking-tight">kodama</h1>
      <p
        role="status"
        className={`text-cap uppercase tracking-wide ${
          listening ? "text-accent-dot" : "text-text-soft"
        }`}
      >
        {listening ? "Listening" : "Idle"}
      </p>
    </main>
  );
}

export default App;
