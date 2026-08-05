import React from "react";
import ReactDOM from "react-dom/client";
import { CaptureOverlayPill } from "./components/capture/CaptureOverlayPill";
import { startContrast } from "./contrast";
import { startReduceMotion } from "./reduceMotion";
import { applyTheme, startThemeSync } from "./theme";
import "./index.css";

// The overlay is its own webview, so it repeats the theme and token bootstrap
// `main.tsx` does — it does not share the main window's runtime. No
// NavigationProvider/AppShell: this window is a single pill, nothing else.
// Paint in the OS theme immediately, then correct to the stored preference
// once it has been read. Doing it the other way round would flash.
applyTheme("system");
startThemeSync();
// The in-app display overrides, alongside the theme: same class of one-time
// document bootstrap, and every window needs them (src/contrast.ts,
// src/reduceMotion.ts).
startContrast();
startReduceMotion();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <CaptureOverlayPill />
  </React.StrictMode>,
);
