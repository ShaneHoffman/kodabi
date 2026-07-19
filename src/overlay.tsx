import React from "react";
import ReactDOM from "react-dom/client";
import { CaptureOverlayPill } from "./components/CaptureOverlayPill";
import { applyTheme } from "./theme";
import "./fonts";
import "./index.css";

// The overlay is its own webview, so it repeats the theme/fonts/token bootstrap
// `main.tsx` does — it does not share the main window's runtime. No
// NavigationProvider/AppShell: this window is a single pill, nothing else.
applyTheme("system");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <CaptureOverlayPill />
  </React.StrictMode>,
);
