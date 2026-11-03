// Entry point. Picks up `?token=` before anything talks to the engine and sets the theme before the first paint. The
// app sits behind the Connect card (app/ConnectScreen.tsx), shown instead of it while the engine refuses the token.
// OpenEtch Design (mode.ts) has no engine and no token: it installs its in-browser backend instead
// (designmode/backend.ts) and marks the page `data-design-only` (styles, and the device harness's checks).

// Design tokens first, so every other stylesheet (styles.css, the components' own) can use them.
import "./tokens.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { initAuthToken } from "./api/client.ts";
import { App } from "./App.tsx";
import { ConnectGate } from "./app/ConnectScreen.tsx";
import { CrashScreen } from "./app/CrashScreen.tsx";
import { GuideStandalone } from "./guide/GuideHost.tsx";
import { isStandalone } from "./guide/route.ts";
import { getPrefs } from "./workspace/prefs.ts";
import { resolveTheme } from "./workspace/theme.ts";
import { DESIGN_ONLY } from "./mode.ts";
import "./styles.css";

if (DESIGN_ONLY) {
  document.documentElement.dataset.designOnly = "";
  void import("./designmode/backend.ts").then((m) => m.installDesignBackend());
} else {
  initAuthToken();
}

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
if (root && isStandalone()) {
  // `?guide` (the guide's Open in new tab): the user guide alone, without the engine connection or the canvas.
  document.title = "OpenEtch user guide";
  createRoot(root).render(
    <StrictMode>
      <CrashScreen>
        <GuideStandalone />
      </CrashScreen>
    </StrictMode>,
  );
} else if (root) {
  createRoot(root).render(
    <StrictMode>
      <CrashScreen>
        {DESIGN_ONLY ? (
          <App />
        ) : (
          <ConnectGate>
            <App />
          </ConnectGate>
        )}
      </CrashScreen>
    </StrictMode>,
  );
}
