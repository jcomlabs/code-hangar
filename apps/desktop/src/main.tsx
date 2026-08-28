import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./ErrorBoundary";
import { readVisualAcceptanceState } from "./visualAcceptance";
import "./styles.css";

const storedTheme = window.localStorage.getItem("codehangar:theme-mode");
document.documentElement.setAttribute("data-theme", storedTheme === "oled" ? "oled" : "light");
const visualAcceptanceState = readVisualAcceptanceState();
if (visualAcceptanceState !== "default") {
  document.documentElement.setAttribute("data-acceptance-state", visualAcceptanceState);
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
