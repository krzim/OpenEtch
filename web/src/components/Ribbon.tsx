// STAGE A STUB (owner: P2). The ribbon of docs/ux/ui-plan-2026-11.md P2: groups that collapse into `GROUP ▾` buttons by
// priority so nothing scrolls (horizontal), the tablet's vertical tool rail, and the phone's tool strip. The
// signatures below are final; until P2 lands both render their children unchanged.

import type { ReactNode } from "react";
import "./ribbon.css";

export type RibbonOrientation = "horizontal" | "vertical" | "strip";

export interface RibbonProps {
  /** horizontal: the desktop ribbon; vertical: the tablet tool rail; strip: the phone's tool strip. */
  orientation: RibbonOrientation;
  children?: ReactNode;
  className?: string;
}

export function Ribbon(p: RibbonProps) {
  return <>{p.children}</>;
}

export interface RibbonGroupProps {
  /** Stable id ("create", "modify", "arrange", "insert", "inspect", "laser"). */
  id: string;
  /** Caption under the group (and the collapsed `GROUP ▾` button's label). */
  label: string;
  /** Icon name (ui.tsx ICONS) for the collapsed button and the vertical rail. */
  icon: string;
  /** Higher stays inline longer; the lowest-priority groups collapse first. */
  priority: number;
  /** Never collapses. */
  pinned?: boolean;
  children?: ReactNode;
}

export function RibbonGroup(p: RibbonGroupProps) {
  return <>{p.children}</>;
}
