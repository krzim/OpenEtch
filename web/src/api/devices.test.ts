// Unit tests for api/devices.ts: the engine's device answers, the active device and the canvas workspace, when
// switching or removing a device is allowed, labels, and the body cut out of a prefs.ini or .lbdev before an import.
// The device files below are small synthetic fixtures (not anyone's real settings); the "secrets" in them are made up.

import assert from "node:assert/strict";
import { test } from "node:test";
import * as D from "./devices.ts";
import { findDevice, lbrnImportField } from "./lightburn.ts";
import type { MachineStatus } from "./types.ts";

const B6 = {
  id: "commarker-b6",
  name: "ComMarker B6 MOPA 60W (EzCad)",
  display_name: "ComMarker B6 MOPA 60W (EzCad)",
  source: "builtin",
  kind: "jcz_galvo",
  lightburn_type: null,
  workspace_mm: [153.75, 153.75],
  origin: "center",
  controllable: true,
};
const JCZ = {
  id: "jczfiber-unknown",
  name: "JCZFiber",
  display_name: "JCZFiber (Unknown)",
  source: "lightburn-prefs",
  kind: "jcz_galvo",
  lightburn_type: "LibUSB",
  workspace_mm: [153.8000030517578, 153.8000030517578],
  origin: "center",
  controllable: true,
};
const WAZER = {
  id: "wazer",
  name: "Custom GCode",
  display_name: "WAZER",
  source: "lbdev",
  kind: "gantry",
  lightburn_type: "Serial",
  workspace_mm: [500, 300],
  origin: "top_left",
  controllable: false,
};
const LIST = { active: "commarker-b6", devices: [B6, JCZ, WAZER] };

// ---- parsing ------------------------------------------------------------------------------------------------------

test("device list: rows keep their order; malformed rows and duplicate ids are dropped", () => {
  const l = D.deviceList({ ...LIST, devices: [B6, { name: "no id", workspace_mm: [1, 1] }, { id: "x" }, JCZ, { ...JCZ }, WAZER, null, 7] });
  assert.deepEqual(
    l.devices.map((d) => d.id),
    ["commarker-b6", "jczfiber-unknown", "wazer"],
  );
  assert.equal(l.active, "commarker-b6");
  assert.deepEqual(l.devices[2].workspace_mm, [500, 300]);
  assert.equal(l.devices[2].controllable, false);
});

test("device list: an active id that is not listed becomes null; garbage is an empty list", () => {
  assert.equal(D.deviceList({ ...LIST, active: "gone" }).active, null);
  assert.deepEqual(D.deviceList(null), { active: null, devices: [] });
  assert.deepEqual(D.deviceList("nope"), { active: null, devices: [] });
});

test("device summary: defaults for missing fields; controllable only when the engine says so", () => {
  const d = D.deviceSummary({ id: "a", width_mm: 300, height_mm: 200 });
  assert.ok(d);
  assert.equal(d.name, "a");
  assert.equal(d.display_name, "a");
  assert.equal(d.kind, "unknown");
  assert.equal(d.origin, "bottom_left");
  assert.equal(d.controllable, false);
  assert.deepEqual(d.workspace_mm, [300, 200]);
  // An unknown kind is "unknown"; a galvo without an origin is centred; field_mm alone is a square.
  const g = D.deviceSummary({ id: "g", kind: "jcz_galvo", field_mm: 110, controllable: "yes" });
  assert.ok(g);
  assert.equal(g.origin, "center");
  assert.deepEqual(g.workspace_mm, [110, 110]);
  assert.equal(g.controllable, false);
  assert.equal(D.deviceSummary({ id: "k", kind: "plasma", workspace_mm: [1, 1] })?.kind, "unknown");
  assert.equal(D.deviceSummary({ id: "z", workspace_mm: [0, 10] }), null);
});

test("device details: settings, profile summary, diffs and base, in either shape", () => {
  const d = D.deviceDetails({
    ...JCZ,
    base: "ComMarker B6 MOPA 60W (EzCad)",
    profile: { field_mm: 153.8, max_power_pct: 100, frequency_khz: [20, 4000], timing: { jump_delay_us: 100, deep: { no: 1 } }, rows: [{ no: 1 }] },
    diffs: [{ field: "field_mm", base_value: 153.75, lightburn_value: 153.8, note: "LightBurn field" }, { nope: 1 }],
    settings: [
      { group: "Galvo", label: "COR file", value: "jcz150.cor" },
      { label: "Bounds check", value: true },
      { value: 3 },
    ],
  });
  assert.ok(d);
  assert.equal(d.base, "ComMarker B6 MOPA 60W (EzCad)");
  assert.deepEqual(d.profile, [
    { label: "field_mm", value: 153.8 },
    { label: "max_power_pct", value: 100 },
    { label: "frequency_khz", value: [20, 4000] },
    { label: "timing.jump_delay_us", value: 100 },
  ]);
  assert.deepEqual(
    d.diffs.map((x) => x.field),
    ["field_mm"],
  );
  assert.deepEqual(d.settings, [
    { group: "Galvo", label: "COR file", value: "jcz150.cor" },
    { group: "General", label: "Bounds check", value: true },
  ]);
  // Wrapped in {device}, settings as {group: {label: value}}, profile as a list.
  const w = D.deviceDetails({
    device: WAZER,
    settings: { GCode: { "Baud rate": 115200 }, "Job origin": 2 },
    profile: [{ label: "Name", value: "x" }],
  });
  assert.ok(w);
  assert.deepEqual(w.settings, [
    { group: "GCode", label: "Baud rate", value: 115200 },
    { group: "General", label: "Job origin", value: 2 },
  ]);
  assert.deepEqual(w.profile, [{ label: "Name", value: "x" }]);
  assert.equal(w.base, null);
  // The engine's sections: [{title, rows: [{label, key, value}]}].
  const e = D.deviceDetails({
    ...JCZ,
    settings: [
      { title: "Device", rows: [{ label: "Name", key: "DisplayName", value: "JCZ" }, { key: "Width/Height", value: [150, 150] }, { value: 1 }] },
      { title: "Galvo", rows: [{ label: "Field angle (°)", key: "FieldAngle", value: 0 }] },
    ],
  });
  assert.ok(e);
  assert.deepEqual(e.settings, [
    { group: "Device", label: "Name", value: "JCZ" },
    { group: "Device", label: "Width/Height", value: [150, 150] },
    { group: "Galvo", label: "Field angle (°)", value: 0 },
  ]);
  assert.equal(D.deviceDetails({ id: "no workspace" }), null);
});

test("import result and its toast", () => {
  const r = D.importResult({ added: ["WAZER", { display_name: "UV" }], updated: 1, skipped: [{ name: "Bad", reason: "no size" }, "Other"] });
  assert.deepEqual(r.added, ["WAZER", "UV"]);
  assert.equal(r.updated.length, 1);
  assert.deepEqual(r.skipped, [
    { name: "Bad", reason: "no size" },
    { name: "Other", reason: "" },
  ]);
  assert.deepEqual(D.importMessage(r), { msg: "Added 2 devices, updated 1 device, skipped 2 (Bad: no size, …).", kind: "info" });
  assert.deepEqual(D.importMessage(D.importResult({ skipped: [{ name: "Bad", reason: "no size" }] })), { msg: "Skipped 1 (Bad: no size).", kind: "error" });
  assert.deepEqual(D.importMessage(D.importResult({})), { msg: "No devices were imported.", kind: "error" });
  assert.equal(D.importMessage(D.importResult({ skipped: [{ name: "JCZ", reason: "already imported, unchanged" }] })).kind, "info");
});

// ---- the active device and the canvas -----------------------------------------------------------------------------

test("active device: from the list, else what GET /api/machine reports", () => {
  const list = D.deviceList(LIST);
  assert.equal(D.activeDevice(list)?.display_name, "ComMarker B6 MOPA 60W (EzCad)");
  assert.equal(D.activeDevice({ ...list, active: null }), null);
  assert.equal(D.activeDevice(null), null);
  assert.equal(D.machineDevice({ connected: true, device: WAZER } as MachineStatus)?.id, "wazer");
  assert.equal(D.machineDevice({ connected: true, active_device: JCZ } as MachineStatus)?.id, "jczfiber-unknown");
  assert.equal(D.machineDevice({ connected: true } as MachineStatus), null);
});

test("canvas workspace: a galvo field (square or not), a gantry rectangle, else the machine field", () => {
  const list = D.deviceList(LIST);
  const [b6, jcz, wazer] = list.devices;
  assert.deepEqual(D.canvasWorkspace(b6, 110, 153.75), [153.75, 153.75]);
  assert.deepEqual(D.canvasWorkspace(wazer, 110, 153.75), [500, 300]);
  // A non-square galvo field (separate field height, OTH-009) is drawn as reported.
  assert.deepEqual(D.canvasWorkspace({ ...jcz, workspace_mm: [150, 140] }, null, 153.75), [150, 140]);
  assert.deepEqual(D.canvasWorkspace(null, 110, 153.75), [110, 110]);
  assert.deepEqual(D.canvasWorkspace(null, null, 153.75), [153.75, 153.75]);
  assert.equal(D.workspaceText([153.8000030517578, 153.8000030517578]), "153.8 mm");
  assert.equal(D.workspaceText([500, 300]), "500 × 300 mm");
  // A galvo shows its real field: square within 0.05 mm (LightBurn's 0.1 mm display), else width x height.
  assert.equal(D.deviceSizeText({ kind: "jcz_galvo", workspace_mm: [153.75, 153.8] }), "153.75 mm");
  assert.equal(D.deviceSizeText({ kind: "jcz_galvo", workspace_mm: [153.75, 100] }), "153.75 × 100 mm");
});

// ---- switching and removing ---------------------------------------------------------------------------------------

const idle = { connected: true, runs: [{ id: "r1", state: "finished" }, { id: "r2", state: "pending" }] } as MachineStatus;

test("switching waits for approved, confirming and running runs and for framing; pending runs do not block", () => {
  assert.equal(D.switchBlocker(idle), null);
  assert.equal(D.switchBlocker(null), null);
  for (const state of ["approved", "awaiting_confirm", "running"]) {
    assert.match(D.switchBlocker({ connected: true, runs: [{ id: "r", state }] } as MachineStatus) ?? "", /run is under way/);
  }
  assert.match(D.switchBlocker({ connected: true, running: true } as MachineStatus) ?? "", /run is under way/);
  assert.match(D.switchBlocker({ connected: true, framing: true } as MachineStatus) ?? "", /framing/);
});

test("make active: not the active device, not while busy", () => {
  const list = D.deviceList(LIST);
  const [b6, jcz] = list.devices;
  assert.equal(D.activateBlocker(jcz, list, idle), null);
  assert.match(D.activateBlocker(b6, list, idle) ?? "", /already the active device/);
  assert.match(D.activateBlocker(jcz, list, { connected: true, framing: true } as MachineStatus) ?? "", /framing/);
  assert.equal(D.activateBlocker(null, list, idle), "Select a device.");
});

test("remove: never the built-in or the active device", () => {
  const list = D.deviceList({ ...LIST, active: "jczfiber-unknown" });
  const [b6, jcz, wazer] = list.devices;
  assert.match(D.removeBlocker(b6, list) ?? "", /built-in/);
  assert.match(D.removeBlocker(jcz, list) ?? "", /another device active/);
  assert.equal(D.removeBlocker(wazer, list), null);
  assert.equal(D.removeBlocker(null, list), "Select a device.");
});

test("selection survives list changes: same id, else the active device, else the first", () => {
  const list = D.deviceList(LIST);
  assert.equal(D.keepSelection(list, "wazer"), "wazer");
  assert.equal(D.keepSelection(list, "removed"), "commarker-b6");
  assert.equal(D.keepSelection({ ...list, active: null }, null), "commarker-b6");
  assert.equal(D.keepSelection({ active: null, devices: [] }, "x"), null);
});

test("switch confirmation says what changes; design-only devices say why they cannot run", () => {
  const list = D.deviceList(LIST);
  const [b6, jcz, wazer] = list.devices;
  const toGalvo = D.switchMessage(jcz, b6);
  assert.match(toGalvo, /field \(153\.8 mm\)/);
  assert.match(toGalvo, /re-initialises the laser/);
  const toGantry = D.switchMessage(wazer, b6);
  assert.match(toGantry, /workspace \(500 × 300 mm\)/);
  assert.match(toGantry, /can't run jobs/);
  assert.match(toGantry, /origin differs/);
  assert.equal(D.designOnlyReason(jcz), null);
  assert.equal(D.designOnlyReason(null), null);
  assert.match(D.designOnlyReason(wazer) ?? "", /can't drive a gantry laser \(Custom GCode\)/);
  assert.equal(D.deviceOptionText(wazer), "WAZER · 500 × 300 mm");
});

test("labels", () => {
  assert.equal(D.originLabel("center"), "Centre (galvo)");
  assert.equal(D.originLabel("top_left"), "Rear left");
  assert.equal(D.originLabel("weird"), "weird");
  assert.equal(D.sourceLabel("lbdev"), ".lbdev file");
  assert.equal(D.kindLabel("jcz_galvo"), "JCZ galvo (EzCad2 / LMC)");
  assert.equal(D.isGalvo("galvo_other"), true);
  assert.equal(D.isGalvo("gantry"), false);
});

test("registry devices in the LightBurn dialogs: a file's DeviceName matches display names first", () => {
  const list = D.deviceList({ ...LIST, devices: [B6, { ...JCZ, id: "a", display_name: "Fiber A" }, { ...JCZ, id: "b", display_name: "JCZFiber" }, WAZER] });
  const lb = D.asLightBurnDevices(list.devices);
  // "JCZFiber" is device a's controller name but device b's display name: b wins, as LightBurn files carry display names.
  assert.equal(findDevice(lb, "JCZFiber")?.name, "JCZFiber");
  assert.equal(findDevice(lb, "fiber a")?.name, "Fiber A");
  assert.equal(D.findRegistryDevice(list.devices, "JCZFiber")?.id, "b");
  assert.equal(D.findRegistryDevice(list.devices, "custom gcode")?.id, "wazer");
  assert.equal(D.findRegistryDevice(list.devices, ""), null);
  // The import rule then gives the gantry's workspace and names the active device.
  const f = lbrnImportField({ fileDevice: "WAZER", devices: lb, engineDevice: "Fiber A", engineKind: "active-device", docField: 150, machineField: 150, fallback: 150 });
  assert.deepEqual([f.kind, f.device, f.workspaceMm], ["file-device", "WAZER", [500, 300]]);
  const g = lbrnImportField({ fileDevice: null, devices: lb, engineDevice: "Fiber A", engineKind: "active-device", docField: 150, machineField: 150, fallback: 150 });
  assert.deepEqual([g.kind, g.device, g.fieldMm, g.workspaceMm], ["active-device", "Fiber A", 153.8000030517578, null]);
  // A B6 device: the profile's field is EzCad's 153.75, LightBurn files use LightBurn's 153.8 (the engine's
  // lightburn_field_mm), so the dialogs show what the engine will use.
  const b6 = D.deviceList({ active: "j", devices: [{ ...JCZ, id: "j", workspace_mm: [153.75, 153.75], lightburn_field_mm: 153.8 }, { ...WAZER, lightburn_field_mm: 99 }] });
  assert.deepEqual([b6.devices[0].lightburn_field_mm, b6.devices[1].lightburn_field_mm], [153.8, null]);
  const lb6 = D.asLightBurnDevices(b6.devices);
  assert.deepEqual([lb6[0].field_mm, lb6[0].width_mm, lb6[1].field_mm], [153.8, 153.75, 500]);
  assert.equal(lbrnImportField({ fileDevice: "JCZFiber (Unknown)", devices: lb6, engineDevice: null, docField: null, machineField: 153.75, fallback: 150 }).fieldMm, 153.8);
});

// ---- import body --------------------------------------------------------------------------------------------------

const PREFS = JSON.stringify({
  DefaultDevice: 1,
  RestPendantSecret: "made-up-secret",
  RecentFiles: ["C:/Users/someone/secret.lbrn2"],
  DeviceList: [
    {
      Name: "GRBL",
      DisplayName: "Shop diode",
      Type: "Serial",
      GUID: "abc123",
      Info: "192.0.2.7",
      Width: 400,
      Height: 300,
      MirrorX: false,
      MirrorY: true,
      LastDevLibraryPath: "C:/Users/someone/lib.clb",
      Checklist: "check the fan",
      DefaultCutList: [{ x: 1 }],
      Settings: { BaudRate: 115200, CutOrigin: 2, GCodeBlocks: { home: "$H" }, EndGCode: "M5", ApiAuth: "made-up-token", Sim_MaxSpeedX: 6000 },
    },
    {
      Name: "JCZFiber",
      DisplayName: "JCZFiber (Unknown)",
      Type: "LibUSB",
      Width: 153.8000030517578,
      Height: 153.8000030517578,
      Settings: { CorrectionFile: "C:\\Users\\someone\\EzCad\\jcz150.cor", Galvo_1_is_X: false, FrameSpeed: 3000, Laser_Type: 3, Nested: { a: 1 } },
    },
    "garbage",
  ],
});

test("import body: only typed device fields leave the browser; paths become file names", () => {
  const b = D.deviceImportBody(PREFS);
  assert.equal(b.kind, "prefs");
  assert.deepEqual(b.names, ["Shop diode", "JCZFiber (Unknown)"]);
  assert.equal(b.defaultIndex, 1);
  const body = JSON.parse(b.body) as { DefaultDevice: number; DeviceList: Record<string, unknown>[] };
  assert.equal(body.DefaultDevice, 1);
  assert.deepEqual(Object.keys(body).sort(), ["DefaultDevice", "DeviceList"]);
  assert.deepEqual(body.DeviceList[0], {
    Name: "GRBL",
    DisplayName: "Shop diode",
    Type: "Serial",
    Width: 400,
    Height: 300,
    MirrorX: false,
    MirrorY: true,
    LastDevLibraryPath: "lib.clb",
    Settings: { BaudRate: 115200, CutOrigin: 2, Sim_MaxSpeedX: 6000 },
  });
  assert.deepEqual(body.DeviceList[1].Settings, { CorrectionFile: "jcz150.cor", Galvo_1_is_X: false, FrameSpeed: 3000, Laser_Type: 3 });
  for (const secret of ["made-up-secret", "made-up-token", "192.0.2.7", "abc123", "someone", "check the fan", "$H", "M5"]) {
    assert.ok(!b.body.includes(secret), `the body leaks ${secret}`);
  }
});

test("import body: a .lbdev is the same shape without DefaultDevice", () => {
  const lbdev = JSON.stringify({ DeviceList: [{ Name: "Custom GCode", DisplayName: "WAZER", Type: "Serial", Width: 500, Height: 300, MirrorY: true }] });
  const b = D.deviceImportBody(`\ufeff${lbdev}`);
  assert.equal(b.kind, "lbdev");
  assert.equal(b.defaultIndex, null);
  assert.deepEqual(b.names, ["WAZER"]);
  assert.deepEqual(JSON.parse(b.body), { DeviceList: [{ Name: "Custom GCode", DisplayName: "WAZER", Type: "Serial", Width: 500, Height: 300, MirrorY: true }] });
});

test("import body: refuses what is not a LightBurn device file", () => {
  assert.throws(() => D.deviceImportBody("<xml/>"), /not a LightBurn device file/);
  assert.throws(() => D.deviceImportBody("[1,2]"), /not a LightBurn device file/);
  assert.throws(() => D.deviceImportBody(JSON.stringify({ Other: 1 })), /no LightBurn device list/);
  assert.throws(() => D.deviceImportBody(JSON.stringify({ DeviceList: [1, "x"] })), /lists no LightBurn devices/);
  assert.equal(D.importAddParam(null), "all");
  assert.equal(D.importAddParam(""), "all");
  assert.equal(D.importAddParam("WAZER"), "WAZER");
});

test("a GRBL device form becomes the engine's profile, typing slips caught first", () => {
  const ok = D.grblDeviceBody({ ...D.GRBL_FORM_DEFAULT, displayName: " Diode ", port: "COM5 ", widthMm: "410", heightMm: "400.5", homing: true, dynamicPower: false });
  assert.ok("body" in ok);
  assert.deepEqual(ok.body, {
    display_name: "Diode",
    profile: {
      port: "COM5",
      baud: 115200,
      work_area_mm: [410, 400.5],
      origin: "bottom_left",
      homing: true,
      s_max: 1000,
      max_feed_mm_min: 6000,
      frame_feed_mm_min: 3000,
      dynamic_power: false,
    },
  });
  for (const bad of [{ displayName: "" }, { baud: "fast" }, { widthMm: "0" }, { heightMm: "" }, { sMax: "-1" }, { maxFeedMmMin: "x" }]) {
    assert.ok("error" in D.grblDeviceBody({ ...D.GRBL_FORM_DEFAULT, ...bad }), JSON.stringify(bad));
  }
  assert.equal(D.kindLabel("grbl"), "GRBL gantry");
  assert.equal(D.isGalvo("grbl"), false);
  assert.equal(D.deviceSummary({ id: "g", name: "GRBL", display_name: "Diode", kind: "grbl", workspace_mm: [400, 300], origin: "bottom_left", controllable: true })?.kind, "grbl");
});
