// LightBurn's Variable Text Formatting codes for one Text Mode, as a collapsible table under the mode picker.

import type { VarMode } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { VAR_HELP } from "./textModel.ts";

export function VarHelp(p: { mode: VarMode }) {
  const t = useT();
  const h = VAR_HELP[p.mode];
  return (
    <details className="tx-help">
      <summary>{t("text.codes")}</summary>
      <table>
        <tbody>
          {h.rows.map(([code, meaning]) => (
            <tr key={code}>
              <td>
                <code>{code}</code>
              </td>
              <td>{meaning}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="hint">
        {t("text.codesExample")} <code>{h.example}</code>. {h.note}
      </p>
    </details>
  );
}
