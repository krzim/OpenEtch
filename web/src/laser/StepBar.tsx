// Beginner Mode's step bar in the Laser window (owner decision 10: the step bar only, this pass): 1 Preview -> 2 Frame
// -> 3 Start, the next step highlighted with a one-line hint.

import { useT } from "../workspace/usePrefs.ts";
import { beginnerSteps, STEPS, type StepInput } from "./steps.ts";
import "./laser.css";
import { HelpLink } from "../guide/HelpLink.tsx";

export function StepBar(p: StepInput) {
  const t = useT();
  const s = beginnerSteps(p);
  const label = { preview: t("lz.stepPreview"), frame: t("lz.stepFrame"), start: t("lz.stepStart") };
  const hint =
    s.current === "burning"
      ? t("lz.stepRunningHint")
      : s.current === "preview"
        ? t("lz.stepPreviewHint")
        : s.current === "frame"
          ? t("lz.stepFrameHint")
          : t("lz.stepStartHint");
  return (
    <div className="step-bar" role="group" aria-label={t("lz.steps")}>
      <HelpLink to="running-a-job#steps-to-burn-beginner-mode" topic={t("lz.steps")} />
      <ol className="step-bar-list">
        {STEPS.map((id, i) => {
          const cur = s.current === id;
          return (
            <li key={id} className={`step${s.done[id] ? " done" : ""}${cur ? " current" : ""}`} aria-current={cur ? "step" : undefined}>
              <span className="step-n" aria-hidden="true">
                {s.done[id] && !cur ? "✓" : i + 1}
              </span>
              <span className="step-label">{label[id]}</span>
            </li>
          );
        })}
      </ol>
      <p className="step-hint" role="status">
        {hint}
      </p>
    </div>
  );
}
