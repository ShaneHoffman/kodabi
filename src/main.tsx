import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { startReduceMotion } from "./reduceMotion";
import { applyTheme, startThemeSync } from "./theme";
import "./fonts";
import "./index.css";

// Paint in the OS theme immediately, then correct to the stored preference
// once it has been read. Doing it the other way round would flash.
applyTheme("system");
startThemeSync();
// The in-app reduce-motion override, alongside the theme: same class of
// one-time document bootstrap, and every window needs it (src/reduceMotion.ts).
startReduceMotion();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
