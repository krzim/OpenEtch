// The in-page layout checker of the device harness (web/e2e/README.md). `checkLayout` is self-contained: the harness
// sends its source to the page (Runtime.evaluate) and gets plain JSON back. It looks at the UI a person can see and
// touch, and reports:
//
// - stop:       the STOP button is missing, partly off screen, or not hit-testable at its centre and corners;
// - overlap:    two visible controls or floating panels whose boxes intersect, neither containing the other in the
//               DOM, where the one on top at the shared area is not an intended cover layer (ALLOWED_COVERS);
// - offscreen:  a control or floating panel partly or wholly outside the viewport (after its scroll containers clip it:
//               a row scrolled out of a list is not reported);
// - safe-area:  a control or panel inside the safe-area bands the device reports (phones: notch, home indicator);
// - small:      on touch devices, an enabled control smaller than 44 x 44 CSS px (Apple HIG / WCAG 2.5.5).
//
// "Intended layers" (documented allowlist): an open menu, context menu, list box, popover, tooltip, modal dialog or its
// backdrop, the phone's bottom sheet and a tablet dock overlay may cover anything below them, as long as the covered
// control is under them (the cover is on top at every sample point of the shared area). A covered control that pokes
// out above a cover (a floating button over the sheet) is still an overlap. Floating wrappers without a background
// or border (pure layout boxes) are not panels: their children are checked instead.

/** Runs in the page. `opts`: { touch: boolean, safe: {top,right,bottom,left}, minTarget?: number }. */
export function checkLayout(opts) {
  const o = { minTarget: 44, ...opts };
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const INTERACTIVE =
    "button, a[href], input:not([type=hidden]), select, textarea, [role=button], [role=menuitem], [role=menuitemcheckbox], [role=menuitemradio], [role=tab], [role=option], [role=slider], [role=switch], [role=checkbox], [contenteditable=true]";
  const FLOAT_ROLES = new Set(["toolbar", "dialog", "alertdialog", "menu", "listbox", "status", "alert", "tooltip", "group", "region"]);
  const FLOAT_CLASS = /(^|[\s-])(vp-overlay|vp-warn|toast|toasts|sheet|bottom-sheet|popover|popup|pop|menu|ctx|hud|touch-mods|touch-actions|banner|strip|pill|chip|float|overlay|drawer|dropdown|tooltip|modal|dialog|palette|nudge|timeline|player|bar)([\s-]|$)/i;
  // Cover layers, by rank: a layer may cover anything of a lower rank (a menu over the sheet, the sheet over the canvas
  // overlays). Rank 2: menus, pop-ups, tooltips, modal dialogs; rank 1: the phone's bottom sheet, tablet dock overlays.
  const COVERS_2 =
    "[role=menu], [role=listbox], [role=tooltip], [role=dialog][aria-modal=true], [role=alertdialog], .menu, .ctx-menu, .context-menu, .popover, .modal, .modal-backdrop, .dialog-backdrop, .ws-modal, .dropdown, .cmd-search, .ribbon-flyout";
  const COVERS_1 = ".bottom-sheet, .dock-overlay, aside.panel[data-overlay]";
  // Full-screen dimmers behind modal dialogs are not panels (what sits above them is the dialog or the raised STOP).
  const BACKDROP = ".modal-backdrop, [class*=backdrop]";
  const ALLOWED_COVERS = `${COVERS_2}, ${COVERS_1}`;
  // Pairs that are one control by design (documented allowlist): [top selector, under selector, reason].
  const ALLOWED_PAIRS = [
    [".tl-marker", ".tl-entry", "the history marker sits on the timeline entries"],
    ["[role=menu], [role=menu] *", ".ribbon-flyout", "a ribbon flyout's dropdown menu opens over the flyout's own edge"],
  ];

  const label = (el) => {
    const tag = el.tagName.toLowerCase();
    const role = el.getAttribute("role");
    const cls = typeof el.className === "string" ? el.className.trim().split(/\s+/).filter(Boolean).slice(0, 3).join(".") : "";
    const txt = (el.getAttribute("aria-label") || el.getAttribute("title") || el.textContent || el.getAttribute("placeholder") || "")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 40);
    return `${tag}${role ? `[role=${role}]` : ""}${cls ? `.${cls}` : ""}${txt ? ` "${txt}"` : ""}`;
  };
  const rnd = (r) => ({ x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.right - r.left), h: Math.round(r.bottom - r.top) });
  const inter = (a, b) => ({ left: Math.max(a.left, b.left), top: Math.max(a.top, b.top), right: Math.min(a.right, b.right), bottom: Math.min(a.bottom, b.bottom) });
  const area = (r) => Math.max(0, r.right - r.left) * Math.max(0, r.bottom - r.top);

  /** The element's box after its scroll and clip containers, or null when nothing of it shows. */
  const visibleBox = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return null;
    // The body of a closed <details> keeps its boxes in Chromium but is not shown.
    const closed = el.closest("details:not([open])");
    if (closed && !closed.querySelector(":scope > summary")?.contains(el)) return null;
    let box = { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    let opacity = 1;
    for (let e = el; e && e !== document.documentElement; e = e.parentElement) {
      const cs = getComputedStyle(e);
      if (cs.display === "none" || cs.visibility === "hidden" || cs.visibility === "collapse" || cs.contentVisibility === "hidden") return null;
      if (e.hasAttribute("inert") || e.getAttribute("aria-hidden") === "true") return null;
      opacity *= parseFloat(cs.opacity || "1");
      if (opacity < 0.05) return null;
      if (e !== el && (cs.overflowX !== "visible" || cs.overflowY !== "visible" || cs.clipPath !== "none")) {
        const c = e.getBoundingClientRect();
        box = inter(box, { left: c.left, top: c.top, right: c.right, bottom: c.bottom });
        if (box.right - box.left < 1 || box.bottom - box.top < 1) return null;
      }
      if (cs.position === "fixed") break;
    }
    return box;
  };
  const hasSurface = (el) => {
    const cs = getComputedStyle(el);
    const bg = cs.backgroundColor;
    const solidBg = bg && !/rgba\(.*,\s*0\)$/.test(bg) && bg !== "transparent";
    return solidBg || cs.backgroundImage !== "none" || cs.boxShadow !== "none" || parseFloat(cs.borderTopWidth) > 0;
  };

  // --- collect -------------------------------------------------------------------------------------------------
  const items = [];
  const seen = new Set();
  const add = (el, kind) => {
    if (seen.has(el)) return;
    const box = visibleBox(el);
    if (!box) return;
    seen.add(el);
    items.push({ el, kind, box, label: label(el) });
  };
  for (const el of document.querySelectorAll(INTERACTIVE)) {
    if (el.closest("svg") && el.tagName.toLowerCase() !== "svg") continue;
    add(el, "control");
  }
  for (const el of document.body.querySelectorAll("*")) {
    if (seen.has(el) || el instanceof SVGElement) continue;
    const cs = getComputedStyle(el);
    if (cs.position !== "fixed" && cs.position !== "absolute") continue;
    if (el.matches(BACKDROP)) continue;
    const role = el.getAttribute("role");
    const cls = typeof el.className === "string" ? el.className : "";
    if (!(role && FLOAT_ROLES.has(role)) && !FLOAT_CLASS.test(cls) && !el.querySelector(INTERACTIVE)) continue;
    const r = el.getBoundingClientRect();
    if (r.width < 16 || r.height < 16) continue;
    if (!hasSurface(el)) continue;
    // Big absolutely placed surfaces (the canvas, a docked column) are the page, not floating panels; cover layers
    // (sheets, dialogs, menus) count at any size.
    if (r.width * r.height > 0.4 * vw * vh && !el.matches(ALLOWED_COVERS)) continue;
    // The canvas viewport is the page at any size (with the user guide beside it, it can be under 40 % of the window).
    if (el.matches(".viewport")) continue;
    add(el, "panel");
  }

  // --- STOP ------------------------------------------------------------------------------------------------------
  const problems = [];
  const stops = [...document.querySelectorAll(".btn-stop")].filter((e) => visibleBox(e));
  if (!stops.length) problems.push({ type: "stop", severity: "critical", detail: "no visible STOP button" });
  for (const s of stops) {
    const r = s.getBoundingClientRect();
    const pts = [
      [r.left + r.width / 2, r.top + r.height / 2],
      [r.left + 4, r.top + 4],
      [r.right - 4, r.top + 4],
      [r.left + 4, r.bottom - 4],
      [r.right - 4, r.bottom - 4],
    ];
    const blocked = [];
    for (const [x, y] of pts) {
      if (x < 0 || y < 0 || x >= vw || y >= vh) {
        blocked.push(`(${Math.round(x)},${Math.round(y)}) off screen`);
        continue;
      }
      const hit = document.elementFromPoint(x, y);
      if (!hit || !(s === hit || s.contains(hit))) blocked.push(`(${Math.round(x)},${Math.round(y)}) under ${hit ? label(hit) : "nothing"}`);
    }
    if (blocked.length) problems.push({ type: "stop", severity: "critical", a: label(s), box: rnd(r), detail: `STOP not hit-testable: ${blocked.join("; ")}` });
  }

  // The page itself zoomed (a pinch that the browser took): the visual viewport no longer shows the whole layout, and
  // STOP may be off screen.
  const vv = window.visualViewport;
  if (vv && Math.abs(vv.scale - 1) > 0.01) {
    const stopOut = stops.some((st) => {
      const r = st.getBoundingClientRect();
      return r.left < vv.offsetLeft || r.top < vv.offsetTop || r.right > vv.offsetLeft + vv.width || r.bottom > vv.offsetTop + vv.height;
    });
    problems.push({
      type: "page-zoom",
      severity: stopOut ? "critical" : "high",
      detail: `the page is zoomed to ${vv.scale.toFixed(2)}x (visual viewport ${Math.round(vv.width)}x${Math.round(vv.height)} at ${Math.round(vv.offsetLeft)},${Math.round(vv.offsetTop)})${stopOut ? ": STOP is off screen" : ""}`,
    });
  }

  // --- pairs -----------------------------------------------------------------------------------------------------
  const owner = (hit, a, b) => (hit && (a.el === hit || a.el.contains(hit)) ? "a" : hit && (b.el === hit || b.el.contains(hit)) ? "b" : null);
  const samples = (r) => {
    const w = r.right - r.left;
    const h = r.bottom - r.top;
    const pts = [[r.left + w / 2, r.top + h / 2]];
    if (w > 6 && h > 6) pts.push([r.left + w * 0.2, r.top + h * 0.2], [r.right - w * 0.2, r.top + h * 0.2], [r.left + w * 0.2, r.bottom - h * 0.2], [r.right - w * 0.2, r.bottom - h * 0.2]);
    return pts.filter(([x, y]) => x >= 0 && y >= 0 && x < vw && y < vh);
  };
  const rank = (el) => (el.closest(COVERS_2) ? 2 : el.closest(COVERS_1) ? 1 : 0);
  let layered = 0;
  for (let i = 0; i < items.length; i++) {
    for (let j = i + 1; j < items.length; j++) {
      const a = items[i];
      const b = items[j];
      if (a.el.contains(b.el) || b.el.contains(a.el)) continue;
      const x = inter(a.box, b.box);
      const w = x.right - x.left;
      const h = x.bottom - x.top;
      if (w < 2 || h < 2) continue;
      const pts = samples(x);
      if (!pts.length) continue;
      const tops = pts.map(([px, py]) => owner(document.elementFromPoint(px, py), a, b));
      const nA = tops.filter((t) => t === "a").length;
      const nB = tops.filter((t) => t === "b").length;
      if (nA === 0 && nB === 0) continue; // both under something else (a modal over both)
      const top = nA >= nB ? a : b;
      const under = top === a ? b : a;
      // An intended cover on top of what it covers.
      if (rank(top.el) > rank(under.el)) {
        layered++;
        continue;
      }
      if (ALLOWED_PAIRS.some(([t, u]) => top.el.matches(t) && under.el.matches(u))) continue;
      // Both inside the same cover layer that is itself a scrolling list: rows touching by a pixel are not reported.
      if (w * h < 6) continue;
      // A toast goes away by itself: what it covers is reported, one step lower.
      const sev = top.el.closest(".toast, .toasts") ? "medium" : a.kind === "panel" && b.kind === "panel" ? "medium" : "high";
      problems.push({
        type: "overlap",
        severity: sev,
        a: top.label,
        b: under.label,
        aBox: rnd(top.box),
        bBox: rnd(under.box),
        area: rnd(x),
        detail: `${top.label} lies over ${under.label} (${Math.round(w)}x${Math.round(h)} px)`,
      });
    }
  }

  // --- viewport, safe area, target size --------------------------------------------------------------------------
  const safe = { top: 0, right: 0, bottom: 0, left: 0, ...(o.safe || {}) };
  const small = [];
  for (const it of items) {
    const b = it.box;
    const out = b.left < -0.5 || b.top < -0.5 || b.right > vw + 0.5 || b.bottom > vh + 0.5;
    if (out) {
      const whole = b.right <= 0 || b.bottom <= 0 || b.left >= vw || b.top >= vh;
      problems.push({ type: "offscreen", severity: it.kind === "panel" || !whole ? "medium" : "low", a: it.label, box: rnd(b), detail: `${it.label} ${whole ? "wholly" : "partly"} outside the ${vw}x${vh} viewport` });
    }
    const bands = [];
    if (safe.top && b.top < safe.top - 0.5 && b.bottom > 0) bands.push("top");
    if (safe.bottom && b.bottom > vh - safe.bottom + 0.5 && b.top < vh) bands.push("bottom");
    if (safe.left && b.left < safe.left - 0.5 && b.right > 0) bands.push("left");
    if (safe.right && b.right > vw - safe.right + 0.5 && b.left < vw) bands.push("right");
    if (bands.length) problems.push({ type: "safe-area", severity: "medium", a: it.label, box: rnd(b), detail: `${it.label} reaches into the ${bands.join("+")} safe-area band` });
    if (o.touch && it.kind === "control") {
      let r = it.el.getBoundingClientRect();
      // A check box or radio button is tapped through its label too: the label's box is the target when larger.
      if (it.el.matches?.("input[type=checkbox], input[type=radio]")) {
        const lab = it.el.closest("label") ?? (it.el.id ? document.querySelector(`label[for="${CSS.escape(it.el.id)}"]`) : null);
        const lr = lab?.getBoundingClientRect();
        if (lr && lr.width * lr.height > r.width * r.height) r = lr;
      }
      // A control scrolled half out of its list keeps its real size: measure the element, not the clipped box.
      // A disabled control is not a target (nothing happens on a tap), so its size is not a problem: the ruler corner,
      // for one, is only the unit label until there are guides to clear, and grows to 44 px once it is enabled.
      const disabled = it.el.disabled || it.el.getAttribute("aria-disabled") === "true";
      if (!disabled && (r.width < o.minTarget - 0.5 || r.height < o.minTarget - 0.5)) small.push({ a: it.label, box: rnd(r) });
    }
  }
  if (small.length)
    problems.push({
      type: "small",
      severity: "low",
      count: small.length,
      detail: `${small.length} touch target(s) under ${o.minTarget} px`,
      items: small.slice(0, 60).map((s) => `${s.a} ${s.box.w}x${s.box.h}`),
    });

  return {
    viewport: { w: vw, h: vh, dpr: window.devicePixelRatio },
    layout: {
      layout: document.documentElement.dataset.layout,
      pointer: document.documentElement.dataset.pointer,
      density: document.documentElement.dataset.density,
      theme: document.documentElement.dataset.theme,
    },
    counts: { controls: items.filter((i) => i.kind === "control").length, panels: items.filter((i) => i.kind === "panel").length, layered },
    problems,
  };
}

/** Run the checker in a cdp.mjs Page. */
export async function runCheck(page, opts) {
  return page.evaluate(checkLayout, opts);
}
