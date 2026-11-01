// Keyboard shortcuts: the cheat sheet (F1 or "?") and, in Edit mode, LightBurn's Edit Hotkeys: click "+" and press
// a key to add it, x removes a key, the reset arrow restores an action's defaults. A key used by another action moves
// to this one (the dialog says so). Bindings are stored in this browser (Preferences).

import { useEffect, useMemo, useState } from "react";
import {
  ACTIONS,
  ACTION_BY_ID,
  HOTKEY_GROUPS,
  RESERVED,
  bindChord,
  chordOf,
  chordsFor,
  conflictsOf,
  resetAction,
  unbindChord,
  type HotkeyAction,
} from "./hotkeys.ts";
import { translateDynamic, type MsgKey } from "./i18n.ts";
import { updatePrefs } from "./prefs.ts";
import { setKeyCapture } from "./useKeymap.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { WsModal } from "./WsModal.tsx";

export function HotkeysDialog(p: { onClose: () => void; edit?: boolean }) {
  const t = useT();
  const prefs = usePrefs();
  const [edit, setEdit] = useState(!!p.edit);
  const [filter, setFilter] = useState("");
  const [recording, setRecording] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const overrides = prefs.hotkeys;
  const label = (a: HotkeyAction) => translateDynamic(prefs.language, `hk.${a.id}`, a.label);

  useEffect(() => {
    if (!recording) return;
    setKeyCapture((e) => {
      if (e.key === "Escape") {
        setRecording(null);
        return;
      }
      const chord = chordOf(e);
      if (!chord) return;
      if (RESERVED.has(chord)) {
        setNote(t("hotkeys.reserved", { chord }));
        return;
      }
      const taken = conflictsOf(chord, recording, overrides);
      const fixedHit = taken.find((id) => ACTION_BY_ID.get(id)?.fixed);
      if (fixedHit) {
        setNote(t("hotkeys.fixedConflict", { chord, action: label(ACTION_BY_ID.get(fixedHit) as HotkeyAction) }));
        return;
      }
      const next = bindChord(overrides, recording, chord);
      if (next) {
        updatePrefs({ hotkeys: next });
        setNote(taken.length ? t("hotkeys.moved", { chord, from: taken.map((id) => label(ACTION_BY_ID.get(id) as HotkeyAction)).join(", ") }) : null);
      }
      setRecording(null);
    });
    return () => setKeyCapture(null);
    // label and t change only with the language, which cannot change while recording.
  }, [recording, overrides]);

  const q = filter.trim().toLowerCase();
  const groups = useMemo(
    () =>
      HOTKEY_GROUPS.map((g) => ({
        g,
        actions: ACTIONS.filter((a) => a.group === g).filter(
          (a) => !q || label(a).toLowerCase().includes(q) || chordsFor(a, overrides).some((c) => c.toLowerCase().includes(q)),
        ),
      })).filter((x) => x.actions.length > 0),
    [q, overrides, prefs.language],
  );

  return (
    <WsModal
      title={edit ? t("hotkeys.editTitle") : t("hotkeys.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          {edit && (
            <button className="btn" onClick={() => updatePrefs({ hotkeys: {} })} disabled={Object.keys(overrides).length === 0}>
              {t("hotkeys.resetAll")}
            </button>
          )}
          <span className="tool-spacer" />
          <button className="btn" onClick={() => setEdit((v) => !v)}>
            {edit ? t("hotkeys.done") : t("hotkeys.edit")}
          </button>
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      <div className="hk-top">
        <input className="hk-search" autoFocus type="search" placeholder={t("hotkeys.search")} value={filter} onChange={(e) => setFilter(e.target.value)} aria-label={t("hotkeys.search")} />
        <span className="hint">{edit ? t("hotkeys.editHint") : t("hotkeys.viewHint")}</span>
      </div>
      {note && (
        <p className="hk-note" role="status">
          {note}
        </p>
      )}
      <div className={`hk-groups${edit ? " editing" : ""}`}>
        {groups.map(({ g, actions }) => (
          <section key={g} className="hk-group">
            <h4>{t(`hotkeys.group.${g}` as MsgKey)}</h4>
            <table className="hk-table">
              <tbody>
                {actions.map((a) => {
                  const chords = chordsFor(a, overrides);
                  const changed = a.id in overrides;
                  return (
                    <tr key={a.id} className={changed ? "changed" : undefined}>
                      <td className="hk-label">{label(a)}</td>
                      <td className="hk-keys">
                        {chords.length === 0 && <span className="muted">{t("hotkeys.none")}</span>}
                        {chords.map((c) => (
                          <span key={c} className="hk-chip">
                            <kbd>{c}</kbd>
                            {edit && !a.fixed && (
                              <button className="hk-x" title={t("hotkeys.remove", { chord: c })} aria-label={t("hotkeys.remove", { chord: c })} onClick={() => updatePrefs({ hotkeys: unbindChord(overrides, a.id, c) })}>
                                ×
                              </button>
                            )}
                          </span>
                        ))}
                        {edit && !a.fixed && (
                          <button className={`btn small hk-add${recording === a.id ? " recording" : ""}`} onClick={() => setRecording(recording === a.id ? null : a.id)}>
                            {recording === a.id ? t("hotkeys.pressKey") : "+"}
                          </button>
                        )}
                        {edit && changed && (
                          <button className="btn small" title={t("hotkeys.resetOne")} onClick={() => updatePrefs({ hotkeys: resetAction(overrides, a.id) })}>
                            ↺
                          </button>
                        )}
                        {edit && a.fixed && <span className="muted hk-fixed">{t("hotkeys.fixed")}</span>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </section>
        ))}
      </div>
      <p className="hint">{t("hotkeys.browserNote")}</p>
    </WsModal>
  );
}
