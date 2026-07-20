import React from "react";
import ReactDOM from "react-dom/client";
import { QuickCapture } from "./components/QuickCapture";
import { applyTheme, startThemeSync } from "./theme";
import "./fonts";
import "./index.css";

// The quick-capture window is its own webview, so it repeats the theme/fonts/
// token bootstrap `main.tsx` does — it does not share the main window's runtime.
// No NavigationProvider/AppShell: this window is a single text box, nothing else.
// Paint in the OS theme immediately, then correct to the stored preference
// once it has been read. Doing it the other way round would flash.
applyTheme("system");
startThemeSync();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QuickCapture />
  </React.StrictMode>,
);
