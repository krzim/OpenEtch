// Entry point. Picks up `?token=` before anything talks to the engine.

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { initAuthToken } from "./api/client.ts";
import { App } from "./App.tsx";
import "./styles.css";

initAuthToken();

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
