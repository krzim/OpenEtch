// Entry point. Picks up `?token=` before anything talks to the engine.

// Design tokens first, so every other stylesheet (styles.css, the components' own) can use them.
import "./tokens.css";
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
