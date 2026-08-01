import React from "react";
import ReactDOM from "react-dom/client";
import { PrimitiveGallery } from "./components/dev/PrimitiveGallery";
import { startContrast } from "./contrast";
import { startReduceMotion } from "./reduceMotion";
import { applyTheme } from "./theme";
import "./index.css";

// The dev-only entry for the Grove primitive gallery, served by Vite at
// /gallery.html while `pnpm dev` runs. `gallery.html` is deliberately NOT in
// vite.config.ts's `build.rollupOptions.input`, so the packaged app never
// carries it — Vite serves any root-level .html in dev, and only builds the
// ones the input map names.
//
// No `startThemeSync` and no `./fonts`: this window talks to no backend (the
// sync listens on a Tauri event that does not exist outside the app), and
// Grove's three faces ship with Windows. The ground starts in the OS theme and
// the page's own toggles take it from there.
applyTheme("system");
startContrast();
startReduceMotion();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <PrimitiveGallery />
  </React.StrictMode>,
);
