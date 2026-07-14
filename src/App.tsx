import { useCaptureState } from "./useCaptureState";

function App() {
  const phase = useCaptureState();

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-md bg-bg text-text font-sans">
      <h1 className="font-serif text-display tracking-tight">kodama</h1>
      <p
        className={`text-cap uppercase tracking-wide ${
          phase === "listening" ? "text-accent-dot" : "text-text-soft"
        }`}
      >
        {phase}
      </p>
    </main>
  );
}

export default App;
