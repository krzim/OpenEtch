// Entry point. Picks up `?token=` before anything talks to the engine and sets the theme before the first paint. The
// app sits behind the Connect card (app/ConnectScreen.tsx), shown instead of it while the engine refuses the token.

// Design tokens first, so every other stylesheet (styles.css, the components' own) can use them.
import "./tokens.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { initAuthToken } from "./api/client.ts";
import { App } from "./App.tsx";
import { ConnectGate } from "./app/ConnectScreen.tsx";
import { CrashScreen } from "./app/CrashScreen.tsx";
import { getPrefs } from "./workspace/prefs.ts";
import { resolveTheme } from "./workspace/theme.ts";
import "./styles.css";

initAuthToken();

// The theme before the first paint (styles.css has one dark block keyed on data-theme; usePrefs.ts keeps it current).
try {
  const dark = typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches;
  const theme = resolveTheme(getPrefs().theme, dark);
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
} catch {
  // No matchMedia: the light theme until usePrefs.ts runs.
}

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <CrashScreen>
        <ConnectGate>
          <App />
        </ConnectGate>
      </CrashScreen>
    </StrictMode>,
  );
}
