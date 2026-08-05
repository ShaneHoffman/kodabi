import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { startContrast } from "./contrast";
import { startReduceMotion } from "./reduceMotion";
import { applyTheme, startThemeSync } from "./theme";
import "./index.css";

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
    <App />
  </React.StrictMode>,
);
