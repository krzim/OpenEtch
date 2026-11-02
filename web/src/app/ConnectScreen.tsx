// The "Connect to OpenEtch" card (docs/ux/ui-plan-2026-11.md 1.3, P4): shown instead of a broken UI when the engine
// answers 401 (no access token stored on this device, a rotated token, an app window that cannot see the browser
// tab's storage). Pasting the token, or the pairing link `openetch token` prints, reconnects without a reload.
//
// Mounting (P1): wrap the app once, `<ConnectGate><App/></ConnectGate>` (main.tsx), or gate on `useNeedsToken()` and
// render `<ConnectScreen onConnected/>` yourself. ConnectGate also remounts the app after a reconnect, so every
// request the app made while it was refused runs again, and offers "Use as an app" once on a touch device.
//
// Safety: the card has its own STOP (and the laser.stop keys), because POST /api/machine/stop needs no token: a run
// started before the token went stale can always be stopped from here.

import { Fragment, useCallback, useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { api, needsTokenNow, subscribeNeedsToken, TOKEN_KEY, tokenFromInput, tryToken } from "../api/client.ts";
import { stopToast } from "../api/runs.ts";
import { Icon } from "../components/ui.tsx";
import { chordOf, resolveKey } from "../workspace/hotkeys.ts";
import { getPrefs } from "../workspace/prefs.ts";
import { useT } from "../workspace/usePrefs.ts";
import { offerAppModeOnce } from "./AppModeDialog.tsx";
import "./app.css";

export interface ConnectScreenProps {
  /** The token was accepted: the app can talk to the engine again. */
  onConnected: () => void;
}

export function ConnectScreen({ onConnected }: ConnectScreenProps) {
  const t = useT();
  const [text, setText] = useState("");
  const [remember, setRemember] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stopNote, setStopNote] = useState<string | null>(null);

  const check = useCallback(
    async (token: string | null) => {
      setBusy(true);
      setError(null);
      const r = await tryToken(token, remember);
      setBusy(false);
      if (r.ok) onConnected();
      else if (r.reason === "refused") setError(t("app.connect.refused"));
      else setError(t("app.connect.unreachable", { detail: r.detail ?? "" }));
    },
    [onConnected, remember, t],
  );

  const submit = (raw: string) => {
    const token = tokenFromInput(raw);
    if (!token) {
      setError(t("app.connect.badInput"));
      return;
    }
    setText(token);
    void check(token);
  };

  const stop = useCallback(() => {
    api
      .stop()
      .then((res) => setStopNote(stopToast(res) || t("app.connect.stopSent")))
      .catch((e: unknown) => setStopNote(t("app.connect.stopFailed", { detail: e instanceof Error ? e.message : String(e) })));
  }, [t]);

  // The laser.stop keys (Ctrl+Pause, Ctrl+Cancel, Ctrl+. or the user's own) stop from here too; nothing else here
  // gives Ctrl+. a meaning of its own.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const chord = chordOf(e);
      if (!chord) return;
      const r = resolveKey(chord, getPrefs().hotkeys);
      if (r.kind === "ws" && r.action === "laser.stop") {
        e.preventDefault();
        e.stopImmediatePropagation();
        stop();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [stop]);

  // Another tab of this browser stored a token: try it.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === TOKEN_KEY && e.newValue) void check(null);
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [check]);

  return (
    <div className="connect-screen">
      <header className="connect-bar">
        <span className="brand-mark" aria-hidden="true" />
        <span className="connect-brand">OpenEtch</span>
        <span className="connect-bar-gap" />
        <button type="button" className="btn btn-stop connect-stop" onClick={stop} title={t("top.stopTitle")}>
          <Icon name="stop" /> {t("top.stop")}
        </button>
      </header>
      <main className="connect-main">
        <form
          className="connect-card"
          aria-labelledby="connect-title"
          onSubmit={(e) => {
            e.preventDefault();
            submit(text);
          }}
        >
          <h1 id="connect-title">{t("app.connect.title")}</h1>
          <p className="connect-lead">{t("app.connect.lead")}</p>
          <label className="connect-field">
            <span>{t("app.connect.tokenLabel")}</span>
            <input
              type="text"
              value={text}
              autoFocus
              autoComplete="off"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              enterKeyHint="go"
              placeholder={t("app.connect.tokenPlaceholder")}
              onChange={(e) => setText(e.target.value)}
              onPaste={(e) => {
                const pasted = e.clipboardData.getData("text");
                if (tokenFromInput(pasted)) {
                  e.preventDefault();
                  submit(pasted);
                }
              }}
            />
          </label>
          <label className="connect-check">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
            <span>{t("app.connect.remember")}</span>
          </label>
          {error && (
            <p className="connect-error" role="alert">
              {error}
            </p>
          )}
          <div className="connect-actions">
            <button type="submit" className="btn btn-primary" disabled={busy || !text.trim()}>
              {busy ? t("app.connect.checking") : t("app.connect.connect")}
            </button>
            <button type="button" className="btn" disabled={busy} onClick={() => void check(null)}>
              {t("app.connect.retry")}
            </button>
          </div>
          <section className="connect-where" aria-labelledby="connect-where-title">
            <h2 id="connect-where-title">{t("app.connect.whereTitle")}</h2>
            <p>
              {t("app.connect.where1")} <code>openetch token</code>
            </p>
            <p>{t("app.connect.where2")}</p>
            <p className="connect-muted">{t("app.connect.where3")}</p>
          </section>
        </form>
        <p className="connect-stop-hint" role="status">
          {stopNote ?? t("app.connect.stopHint")}
        </p>
      </main>
    </div>
  );
}

/** The engine refused the stored token (401) or there is none: show ConnectScreen. */
export function useNeedsToken(): boolean {
  return useSyncExternalStore(subscribeNeedsToken, needsTokenNow, needsTokenNow);
}

/**
 * The app behind the Connect card: `children` while the engine accepts the token, the card while it does not. After
 * a reconnect the children mount afresh (everything refetches). Once connected on a touch device, "Use as an app" is
 * offered once.
 */
export function ConnectGate({ children }: { children: ReactNode }) {
  const needs = useNeedsToken();
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    if (!needs) offerAppModeOnce();
  }, [needs]);
  if (needs) return <ConnectScreen onConnected={() => setGeneration((g) => g + 1)} />;
  return <Fragment key={generation}>{children}</Fragment>;
}
