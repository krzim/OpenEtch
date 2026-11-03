---
title: Snapping and guides
order: 8
section: Designing
needs: design
summary: Snap to the points and outlines of other objects or to the grid, place guides to line things up, and measure distances and angles.
---

# Snapping and guides

Snapping and guides help you land exactly on other objects. Their toggles and the **Measure** tool are in the **Inspect** group of the ribbon.

> **Note:** On a Mac, press `Cmd` where this guide says `Ctrl`, and `Option` where it says `Alt`.

## Snapping

Snapping works on the real shapes, as in Fusion 360.

The toggles are in the **Inspect** group:

- **Snap to objects**: centres, quadrants, vertices and endpoints, midpoints, intersections and the nearest point on an outline, of objects, the stock and the field, plus dotted alignment guides.
- **Snap to grid**, with the grid snap distance next to it.
- **Filled view** (`Alt+Shift+W`): draws fill layers as shaded areas. It does not change snapping.

When a snap is found, the canvas shows a small glyph and a label, such as `Center · Rim`. Points win over outlines, outlines over alignment guides, and guides over the grid.

When you drag a selection, it snaps by its own snap point nearest to where you grabbed it.

While dragging:

- Hold `Ctrl` to turn snapping off for this drag.
- Hold `Alt` to snap to guides only.

### Snapping preferences

In Preferences, under **Snapping**, you can set the **Snap distance** (in screen pixels) and switch each kind on or off: **Vertices and endpoints**, **Intersections**, **Centres**, **Quadrants**, **Midpoints**, **Nearest point on outline** and **Alignment guides**. Under **Grid**, you can **Show the grid** and set the **Visual grid spacing**.

Preferences is the gear button in the top bar, or **File > Settings > Preferences…** In the compact and tablet layouts the gear folds into the **More** (⋯) menu in the top bar. On the phone it is **More > Settings > Preferences**.

## Guides

Guides are straight lines you place to line things up. Shapes snap to them.

To add a guide:

1. Drag down from the top ruler for a horizontal guide, or right from the left ruler for a vertical one.
2. Let go where you want it.

To place a guide at an exact position, choose **Arrange > Add guide…** Pick **Vertical (at X)** or **Horizontal (at Y)**, type the position in mm and click **OK**.

To change a guide, double-click it to type a new position, or drag it with the Select tool. On a touch screen, press and hold a guide before you drag it.

To remove a guide, drag it back onto a ruler, or double-click it and click **Delete**. To remove them all, choose **Arrange > Clear guides**.

Guides are saved with the project. They never burn.

## Measure

To measure, click **Measure** (`Alt+M`) in the **Inspect** group, then click two points or drag between them. The bar shows the distance, ΔX, ΔY and the angle. The points snap to objects; `Shift` keeps to 15° steps.

## Common mistakes

- **A shape will not land where you want it.** A stronger snap wins: points over outlines, outlines over guides, guides over the grid. Hold `Alt` to snap to guides only, or `Ctrl` for no snapping.
- **Shapes snap to the grid but the grid is not shown.** **Snap to grid** and **Show the grid** are separate settings. Turn on **Show the grid** in Preferences, under **Grid**.
