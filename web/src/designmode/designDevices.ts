// OpenEtch Design's built-in devices (docs/design-only/plan.md "A Design device picker"): what a design is laid out,
// previewed and timed for. The B6 MOPA (the engine's own built-in profile) is the default, then generic galvo fields of
// the common lens sizes. Seeded by the design backend into GET /api/devices (source "builtin": read-only, Duplicate to
// change the field); devices imported from LightBurn's .lbdev / prefs.ini join them. Pure: `npm test` covers it.

import type { DeviceList, DeviceSummary } from "../api/devices.ts";

/** The default design device: the engine's built-in profile (fixtures/b6-mopa-60w.toml), so ids match. */
export const DEFAULT_DESIGN_DEVICE = "commarker-b6-mopa-60w-ezcad";

/** Generic galvo field sizes, mm (110, 150, 200 and 300 mm lenses). */
export const GENERIC_FIELDS_MM = [110, 150, 200, 300] as const;

function galvo(id: string, display: string, fieldMm: number, lightburnFieldMm: number): DeviceSummary {
  return {
    id,
    name: "JCZFiber",
    display_name: display,
    source: "builtin",
    kind: "jcz_galvo",
    lightburn_type: null,
    workspace_mm: [fieldMm, fieldMm],
    lightburn_field_mm: lightburnFieldMm,
    origin: "center",
    // Nothing is driven from the browser: every device only sets the field, the preview and the time estimate.
    controllable: false,
  };
}

/** The built-in design devices, default first. */
export function designDevices(): DeviceSummary[] {
  return [
    galvo(DEFAULT_DESIGN_DEVICE, "ComMarker B6 MOPA 60W", 153.75, 153.8),
    ...GENERIC_FIELDS_MM.map((mm) => galvo(`generic-galvo-${mm}`, `Generic galvo, ${mm} mm field`, mm, mm)),
  ];
}

/** GET /api/devices of a fresh design backend. */
export function designDeviceList(active: string = DEFAULT_DESIGN_DEVICE): DeviceList {
  const devices: (DeviceSummary & { active: boolean })[] = designDevices().map((d) => ({ ...d, active: d.id === active }));
  return { active, devices };
}
