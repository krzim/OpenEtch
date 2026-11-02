// The Arrange actions (the toolbar's Arrange menu and align strip, Properties' align buttons) and the hotkeys.ts action
// each one is, so every menu that offers them carries the action id: command search then lists each once (it merges
// entries by action id, commandSearch.ts) and the menus show the user's own chord. Pure; tested in commandSearch.test.ts.

export type ArrangeAction =
  | "left"
  | "hcenter"
  | "right"
  | "top"
  | "vcenter"
  | "bottom"
  | "centers"
  | "distH"
  | "distV"
  | "distHc"
  | "distVc"
  | "stock"
  | "field"
  | "group"
  | "ungroup"
  | "flipH"
  | "flipV";

/** The hotkeys.ts action of each Arrange action ("stock", centre on the stock, has none). */
export const ARRANGE_ACTION_ID: Readonly<Record<ArrangeAction, string | undefined>> = {
  left: "arrange.alignLeft",
  hcenter: "arrange.alignHCenter",
  right: "arrange.alignRight",
  top: "arrange.alignTop",
  vcenter: "arrange.alignVCenter",
  bottom: "arrange.alignBottom",
  centers: "arrange.alignCenters",
  distH: "arrange.distHSpaced",
  distHc: "arrange.distHCentered",
  distV: "arrange.distVSpaced",
  distVc: "arrange.distVCentered",
  stock: undefined,
  field: "arrange.centerField",
  group: "arrange.group",
  ungroup: "arrange.ungroup",
  flipH: "arrange.flipH",
  flipV: "arrange.flipV",
};
