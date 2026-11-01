// Unit tests for api/lightburn.ts: the DeviceName of a .lbrn2 file, the field size a LightBurn import uses when nobody
// types one, the engine's LightBurn answers, and the device list cut out of prefs.ini before it is uploaded. The
// prefs.ini below is a small synthetic fixture (not anyone's real settings); the "secrets" in it are made up.

import assert from "node:assert/strict";
import { test } from "node:test";
import * as L from "./lightburn.ts";
import type { MachineStatus } from "./types.ts";

const JCZ: L.LightBurnDevice = {
  name: "JCZFiber",
  display_name: "JCZFiber (Unknown)",
  field_mm: 153.8000030517578,
  width_mm: 153.8000030517578,
  height_mm: 153.8000030517578,
};
const UV: L.LightBurnDevice = { name: "UV", display_name: "UV (110 lens)", field_mm: 110, width_mm: 110, height_mm: 110 };

// ---- DeviceName ---------------------------------------------------------------------------------------------------

test("DeviceName comes from the root element of a LightBurn 2.x file", () => {
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="2.1.02" DeviceName="JCZFiber (Unknown)" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="True">
  <CutSetting type="Cut"><index Value="0"/></CutSetting>
</LightBurnProject>`;
  assert.equal(L.lbrnDeviceName(xml), "JCZFiber (Unknown)");
});

test("DeviceName: LightBurn 1.x files have none; other XML is not a LightBurn project", () => {
  assert.equal(L.lbrnDeviceName(`<?xml version="1.0"?>\n<LightBurnProject AppVersion="1.4.00" FormatVersion="1">`), null);
  assert.equal(L.lbrnDeviceName(`<svg DeviceName="JCZFiber"></svg>`), null);
  assert.equal(L.lbrnDeviceName(""), null);
  assert.equal(L.lbrnDeviceName("not xml at all"), null);
});

test("DeviceName: BOM, comments, single quotes, entities and whitespace around '='", () => {
  const xml = `\ufeff<?xml version="1.0"?>\n<!-- made by hand: DeviceName="Comment" -->\n<LightBurnProject\n  AppVersion='2.0'\n  DeviceName = 'Fiber &amp; Co &#x26; &#60;60W&gt;'>`;
  assert.equal(L.lbrnDeviceName(xml), "Fiber & Co & <60W>");
});

test("DeviceName: only the root start tag counts, and a '>' inside a value does not end it", () => {
  const nested = `<LightBurnProject AppVersion="2.0" FormatVersion="1"><Shape DeviceName="Inner"/></LightBurnProject>`;
  assert.equal(L.lbrnDeviceName(nested), null);
  const gt = `<LightBurnProject Note="a > b" DeviceName="After"><x/></LightBurnProject>`;
  assert.equal(L.lbrnDeviceName(gt), "After");
  // A similarly named attribute is not DeviceName.
  assert.equal(L.lbrnDeviceName(`<LightBurnProject MyDeviceName="X" DeviceNames="Y">`), null);
  // An empty or truncated root tag gives nothing rather than a guess.
  assert.equal(L.lbrnDeviceName(`<LightBurnProject DeviceName="">`), null);
  assert.equal(L.lbrnDeviceName(`<LightBurnProject DeviceName="JCZ`), null);
});

test("readLbrnDeviceName reads only the start of a picked file", async () => {
  const head = `<?xml version="1.0"?>\n<LightBurnProject DeviceName="JCZFiber (Unknown)">`;
  const big = new Blob([head, "<Shape/>".repeat(20_000), "</LightBurnProject>"]);
  assert.ok(big.size > L.LBRN_HEAD_BYTES);
  assert.equal(await L.readLbrnDeviceName(big), "JCZFiber (Unknown)");
  assert.equal(await L.readLbrnDeviceName(new Blob([])), null);
});

// ---- device lookup and the field-size rule --------------------------------------------------------------------------

test("findDevice matches the name or the display name, ignoring case", () => {
  assert.equal(L.findDevice([UV, JCZ], "jczfiber"), JCZ);
  assert.equal(L.findDevice([UV, JCZ], "JCZFIBER (unknown)"), JCZ);
  assert.equal(L.findDevice([UV, JCZ], " UV "), UV);
  assert.equal(L.findDevice([UV, JCZ], "Other"), null);
  assert.equal(L.findDevice([UV, JCZ], null), null);
  assert.equal(L.findDevice([UV, JCZ], ""), null);
});

const base = { devices: [UV, JCZ], engineDevice: "UV", docField: 150, machineField: 153.75, fallback: 153.75 };

test("import field: the file's device first", () => {
  const f = L.lbrnImportField({ ...base, fileDevice: "JCZFiber (Unknown)" });
  assert.deepEqual(f, { kind: "file-device", device: "JCZFiber", fieldMm: 153.8000030517578, unknownFileDevice: null, workspaceMm: null });
  assert.equal(L.fieldSourceText(f), "from your LightBurn device JCZFiber");
});

test("import field: then the engine's LightBurn device, the document, the machine", () => {
  // No DeviceName (LightBurn 1.x file): the engine's device.
  let f = L.lbrnImportField({ ...base, fileDevice: null });
  assert.equal(f.kind, "default-device");
  assert.equal(f.device, "UV");
  assert.equal(f.fieldMm, 110);
  // A device this engine does not know: reported, and the rule goes on.
  f = L.lbrnImportField({ ...base, fileDevice: "Shop CO2" });
  assert.equal(f.kind, "default-device");
  assert.equal(f.unknownFileDevice, "Shop CO2");
  // No LightBurn settings at all: the document's field, then the machine's, then the fallback.
  f = L.lbrnImportField({ ...base, devices: [], engineDevice: null, fileDevice: "JCZFiber (Unknown)" });
  assert.deepEqual(f, { kind: "document", device: null, fieldMm: 150, unknownFileDevice: "JCZFiber (Unknown)", workspaceMm: null });
  f = L.lbrnImportField({ ...base, devices: [], engineDevice: null, fileDevice: null, docField: null });
  assert.deepEqual(f, { kind: "machine", device: null, fieldMm: 153.75, unknownFileDevice: null, workspaceMm: null });
  f = L.lbrnImportField({ ...base, devices: [], engineDevice: null, fileDevice: null, docField: null, machineField: null, fallback: 120 });
  assert.equal(f.fieldMm, 120);
  // An engine device name that matches nothing is skipped.
  f = L.lbrnImportField({ ...base, fileDevice: null, engineDevice: "Gone" });
  assert.equal(f.kind, "document");
});

test("import field: the active OpenEtch device, and a gantry device's workspace", () => {
  const shop: L.LightBurnDevice = { name: "Shop CO2", display_name: "Ruida", field_mm: 600, width_mm: 600, height_mm: 400, kind: "gantry" };
  const galvo: L.LightBurnDevice = { ...UV, kind: "jcz_galvo" };
  let f = L.lbrnImportField({ ...base, devices: [galvo, shop], engineDevice: "UV", engineKind: "active-device", fileDevice: null });
  assert.equal(f.kind, "active-device");
  assert.equal(f.workspaceMm, null);
  assert.equal(L.fieldSourceText(f), "from your active device UV");
  f = L.lbrnImportField({ ...base, devices: [galvo, shop], engineDevice: "UV", engineKind: "active-device", fileDevice: "Shop CO2" });
  assert.equal(f.kind, "file-device");
  assert.deepEqual(f.workspaceMm, [600, 400]);
  // A gantry file and a galvo file never share one size line.
  const g = L.lbrnImportField({ ...base, devices: [galvo, shop], engineDevice: "UV", fileDevice: null });
  assert.equal(L.sharedField([f, g]), null);
  assert.deepEqual(L.parseFieldSource("active device Shop CO2"), { kind: "active-device", device: "Shop CO2" });
});

test("import field: a device without a usable size is skipped", () => {
  const noSize = { ...JCZ, field_mm: null, width_mm: null };
  const f = L.lbrnImportField({ ...base, devices: [noSize, UV], fileDevice: "JCZFiber" });
  assert.equal(f.kind, "default-device");
  assert.equal(f.fieldMm, 110);
});

test("engineLightBurnDevice prefers the device the engine matched, then LightBurn's default", () => {
  const info = L.lightburnInfo({ available: true, source: "file", default_device: "UV", devices: [UV, JCZ] });
  const machine = { connected: true, lightburn: { device: "JCZFiber", field_mm: 153.8, diffs: [] } } as MachineStatus;
  assert.equal(L.engineLightBurnDevice(machine, info), "JCZFiber");
  assert.equal(L.engineLightBurnDevice({ connected: true }, info), "UV");
  assert.equal(L.engineLightBurnDevice(null, null), null);
});

test("sharedField: one line when every file lands on the same size and source", () => {
  const a = L.lbrnImportField({ ...base, fileDevice: "JCZFiber" });
  const b = L.lbrnImportField({ ...base, fileDevice: "jczfiber (unknown)" });
  const c = L.lbrnImportField({ ...base, fileDevice: null });
  assert.equal(L.sharedField([a, b]), a);
  assert.equal(L.sharedField([a, c]), null);
  assert.equal(L.sharedField([]), null);
});

test("parseFieldSource reads the engine's field_source", () => {
  assert.deepEqual(L.parseFieldSource("file device JCZFiber"), { kind: "file-device", device: "JCZFiber" });
  assert.deepEqual(L.parseFieldSource("LightBurn default device JCZFiber (2)"), { kind: "default-device", device: "JCZFiber (2)" });
  assert.deepEqual(L.parseFieldSource("document"), { kind: "document", device: null });
  assert.deepEqual(L.parseFieldSource("machine"), { kind: "machine", device: null });
  assert.deepEqual(L.parseFieldSource("caller"), { kind: "caller", device: null });
  assert.equal(L.parseFieldSource("somewhere"), null);
  assert.equal(L.parseFieldSource(null), null);
});

// ---- engine answers -----------------------------------------------------------------------------------------------

test("lightburnInfo keeps valid devices and only the contract fields", () => {
  const info = L.lightburnInfo({
    available: true,
    source: "uploaded",
    default_device: "JCZFiber",
    devices: [
      { name: "JCZFiber", display_name: "JCZFiber (Unknown)", field_mm: 153.8, width_mm: 153.8, height_mm: 153.8, secret: "x" },
      { display_name: "Only display", width_mm: 110 },
      { field_mm: 100 },
      null,
    ],
  });
  assert.equal(info.available, true);
  assert.equal(info.source, "uploaded");
  assert.equal(info.default_device, "JCZFiber");
  assert.deepEqual(info.devices, [
    { name: "JCZFiber", display_name: "JCZFiber (Unknown)", field_mm: 153.8, width_mm: 153.8, height_mm: 153.8 },
    { name: "Only display", display_name: "Only display", field_mm: 110, width_mm: 110, height_mm: null },
  ]);
  assert.deepEqual(L.lightburnInfo({ available: true, source: "elsewhere", devices: [] }), {
    available: false,
    source: null,
    default_device: null,
    devices: [],
  });
  assert.equal(L.lightburnInfo(null).available, false);
});

test("machineLightBurn reads the machine status's lightburn diffs", () => {
  const m = {
    connected: true,
    lightburn: {
      device: "JCZFiber",
      field_mm: 153.8,
      diffs: [
        { field: "field_mm", base_value: 153.75, lightburn_value: 153.8, note: "same field, rounded" },
        { field: "swap_xy", base_value: true, lightburn_value: false, note: null },
        { base_value: 1 },
      ],
    },
  } as MachineStatus;
  const lb = L.machineLightBurn(m);
  assert.equal(lb?.device, "JCZFiber");
  assert.equal(lb?.diffs.length, 2);
  assert.equal(lb?.diffs[0].note, "same field, rounded");
  assert.equal(L.machineLightBurn({ connected: true }), null);
  assert.equal(L.machineLightBurn({ connected: true, lightburn: null }), null);
  assert.equal(L.formatDiffValue(153.8000030517578), "153.8");
  assert.equal(L.formatDiffValue(false), "No");
  assert.equal(L.formatDiffValue(null), "–");
  assert.equal(L.formatDiffValue("jcz150.cor"), "jcz150.cor");
  assert.equal(L.diffLabel("timing.laser_on_delay_us"), "Timing laser on delay us");
});

// ---- prefs.ini upload ---------------------------------------------------------------------------------------------

/** A synthetic prefs.ini in LightBurn's shape: one galvo device plus unrelated settings and made-up secrets. */
const PREFS = {
  DefaultDevice: 0,
  RecentFiles: ["C:/Users/someone/private.lbrn2"],
  LicenseKey: "FAKE-LICENSE-0000",
  DeviceList: [
    {
      Name: "JCZFiber",
      DisplayName: "JCZFiber (Unknown)",
      Type: "LibUSB",
      Width: 153.8000030517578,
      Height: 153.8000030517578,
      MirrorX: false,
      MirrorY: false,
      GUID: "{00000000-0000-0000-0000-000000000000}",
      RestPendantSecret: "fake-pendant-secret",
      RestAppApiAuths: [{ token: "fake-token" }],
      Macros: [{ name: "m", code: "x" }],
      Settings: {
        CorrectionFile: "C:/Users/someone/EzCad/jcz150.cor",
        CorScaleOverride: false,
        Default_LaserOn_TC: -300,
        Default_JumpSpeed: 4000,
        Galvo_1_is_X: false,
        Galvo_1_Sign: 1,
        Galvo_2_Sign: 1,
        Laser_MaxFreq: 4000,
        Laser_OpenMODelay: 8000,
        MaxSpeed: 7000,
        StartMarkPortBit: 14,
        DockState_Console: "some layout",
        Sim_MaxAccelX: 1,
        Checklist: { items: ["a"] },
      },
    },
    "not a device",
  ],
};

test("devicePrefsBody keeps only the device fields the engine maps", () => {
  const { body, devices } = L.devicePrefsBody(JSON.stringify(PREFS));
  assert.equal(devices, 1);
  const sent = JSON.parse(body);
  assert.deepEqual(Object.keys(sent), ["DefaultDevice", "DeviceList"]);
  assert.equal(sent.DefaultDevice, 0);
  const d = sent.DeviceList[0];
  assert.deepEqual(Object.keys(d).sort(), ["DisplayName", "Height", "MirrorX", "MirrorY", "Name", "Settings", "Type", "Width"]);
  assert.equal(d.Settings.CorrectionFile, "jcz150.cor");
  assert.equal(d.Settings.Default_LaserOn_TC, -300);
  assert.equal(d.Settings.StartMarkPortBit, 14);
  assert.equal(d.Settings.Galvo_1_is_X, false);
  for (const k of ["DockState_Console", "Sim_MaxAccelX", "Checklist"]) assert.ok(!(k in d.Settings), k);
  // Nothing outside the device fields reaches the body.
  for (const leak of ["FAKE-LICENSE", "fake-pendant-secret", "fake-token", "private.lbrn2", "someone", "GUID", "Macros"]) {
    assert.ok(!body.includes(leak), leak);
  }
});

test("devicePrefsBody accepts a BOM and refuses files that are not LightBurn's prefs.ini", () => {
  assert.equal(L.devicePrefsBody(`\ufeff${JSON.stringify({ DeviceList: [{ Name: "A", Width: 100 }] })}`).devices, 1);
  // DefaultDevice is dropped unless it is an index.
  assert.equal(L.devicePrefsBody(JSON.stringify({ DefaultDevice: "x", DeviceList: [{ Name: "A" }] })).body, '{"DeviceList":[{"Name":"A"}]}');
  assert.throws(() => L.devicePrefsBody("[General]\nkey=value"), /not a JSON settings file/);
  assert.throws(() => L.devicePrefsBody("[1, 2]"), /not LightBurn's prefs.ini/);
  assert.throws(() => L.devicePrefsBody('{"Other": 1}'), /no LightBurn device list/);
  assert.throws(() => L.devicePrefsBody('{"DeviceList": [1, "x"]}'), /no devices/);
});

test("devicePrefsBody does not let nested data through an allowed key", () => {
  const { body } = L.devicePrefsBody(JSON.stringify({ DeviceList: [{ Name: { secret: "s" }, Width: 100, Settings: { MaxSpeed: { a: 1 } } }] }));
  assert.equal(body, '{"DeviceList":[{"Width":100,"Settings":{}}]}');
});

// Real LightBurn settings, only when OPENETCH_LIGHTBURN_PREFS lists prefs.ini paths (';' separated). Nothing from the
// files is printed: the test only checks that the upload body keeps the allowed device fields and nothing else.
const realPrefs = (process.env.OPENETCH_LIGHTBURN_PREFS ?? "").split(";").map((p) => p.trim()).filter(Boolean);
test("real prefs.ini files reduce to the device list only", { skip: realPrefs.length === 0 && "set OPENETCH_LIGHTBURN_PREFS" }, async () => {
  const { readFile } = await import("node:fs/promises");
  const deviceKeys = new Set(["Name", "DisplayName", "Type", "Width", "Height", "MirrorX", "MirrorY", "Settings"]);
  const settingKeys = new Set(L.DEVICE_SETTINGS_KEYS);
  for (const path of realPrefs) {
    const { body, devices } = L.devicePrefsBody(await readFile(path, "utf8"));
    assert.ok(devices >= 1, "at least one device");
    const sent = JSON.parse(body) as { DeviceList: Record<string, unknown>[] };
    for (const d of sent.DeviceList) {
      for (const k of Object.keys(d)) assert.ok(deviceKeys.has(k), "unexpected device key");
      for (const k of Object.keys((d.Settings as object) ?? {})) assert.ok(settingKeys.has(k), "unexpected settings key");
      assert.equal(typeof d.Name, "string");
    }
    for (const leak of ["Secret", "Auth", "License", "Recent", "/", "\\"]) assert.ok(!body.includes(leak), "body keeps only device fields");
  }
});

test("labelledFieldChoices keeps the first label for a size and drops unusable sizes", () => {
  const c = L.labelledFieldChoices([
    { mm: 153.8000030517578, label: "JCZFiber" },
    { mm: 153.8, label: "document" },
    { mm: null, label: "machine" },
    { mm: 0, label: "zero" },
    { mm: 110, label: null },
    { mm: 150, label: null },
  ]);
  assert.deepEqual(c, [
    { mm: 153.8000030517578, label: "JCZFiber" },
    { mm: 110, label: null },
    { mm: 150, label: null },
  ]);
});
