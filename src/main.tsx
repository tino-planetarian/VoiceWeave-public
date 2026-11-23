import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import RecordingOverlay from "./RecordingOverlay";
import MemoryPrompt from "./MemoryPrompt";
import { getCurrentWindow } from "@tauri-apps/api/window";

const isOverlay = getCurrentWindow().label === "recording-overlay";
const isMemoryPrompt = getCurrentWindow().label === "memory-prompt";
if (isOverlay) document.documentElement.classList.add("overlay-page");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isOverlay ? <RecordingOverlay /> : isMemoryPrompt ? <MemoryPrompt /> : <App />}
  </React.StrictMode>,
);
