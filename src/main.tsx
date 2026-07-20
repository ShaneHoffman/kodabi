import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, startThemeSync } from "./theme";
import "./fonts";
import "./index.css";

// Paint in the OS theme immediately, then correct to the stored preference
// once it has been read. Doing it the other way round would flash.
applyTheme("system");
startThemeSync();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
