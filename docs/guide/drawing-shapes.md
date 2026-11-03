---
title: Drawing shapes
order: 6
section: Designing
needs: design
summary: Draw rectangles, ellipses, circles, lines, curves, polygons and stars, and change a shape's size and corners in Shape properties.
---

# Drawing shapes

This page covers the drawing tools in the **Create** group: **Rectangle**, **Ellipse**, **Circle**, **Line**, **Pen** and **Polygon**. For where the tools are on a tablet or phone, see [Where the tools are](drawing-and-editing.md#where-the-tools-are). Text and barcodes have their own pages: [Text](text.md) and [Variable text and barcodes](variable-text-and-barcodes.md).

New shapes go on the current layer. To choose the layer first, click it in **Cuts / Layers**.

## Rectangles, ellipses and circles

To draw a rectangle or an ellipse:

1. Click **Rectangle** (`R`) or **Ellipse** (`E`).
2. Drag on the canvas from one corner to the opposite corner.
3. Let go. The shape is selected, ready to move or size.

To draw a circle from its centre:

1. Click **Circle** (`C`).
2. Press where the centre goes and drag outwards.

With any of these tools, a tap or click without a drag places a 20 mm shape.

These keys change a drag while you hold them:

| Key | Effect |
|---|---|
| `Shift` | A square, or a circle with the Ellipse tool |
| `Ctrl` | Draw from the centre (with the Circle tool: snapping off) |
| `Alt` | Snap to guides only |
| `Esc` | Cancel the drag |

On a touch screen, the modifier bar at the bottom left of the canvas has **Square** and **From centre** buttons for the same thing (see [The modifier bar](the-workspace.md#the-modifier-bar)).

> **Note:** With two or more objects selected, `R`, `E`, `C`, `T`, `L` and `B` align the selection instead of choosing a tool. See [Align](arranging-and-arrays.md#align). Single-letter keys work only while the canvas has the focus.

## Shape properties

A rectangle, ellipse, polygon or star you drew stays editable as that shape. Select one shape on its own and the **Shape properties** section shows in the Properties window:

| Shape | Fields |
|---|---|
| Rectangle | **Width**, **Height**, **Corner radius** |
| Ellipse | **Radius X**, **Radius Y** |
| Polygon | **Sides**, **Radius**, **Star**, **Corner radius** |
| Star | **Points**, **Radius**, **Star**, **Inner radius** (%), **Corner radius** |

The shape keeps its centre, rotation and layer. To edit its nodes one by one, convert it to a path first (**Modify > Convert to path**, `Ctrl+Shift+C`), then see [Edit nodes](nodes-and-paths.md#edit-nodes).

The corner radius can be at most half the shorter side. A polygon has 3 to 1000 sides.

## Draw lines and curves

To draw a line or a polyline:

1. Click **Line** (`Ctrl+L`).
2. Click each point in turn.
3. To finish, double-click, press `Enter`, or click **Finish** in the tool bar.
4. To close the shape instead, press `C`, click **Close**, or click the first point again.

While you draw:

- Hold `Shift` to keep the line at 15° steps.
- Press `Backspace`, or click **Undo point**, to remove the last point.
- Press `Esc` to cancel the line.

To draw smooth curves, use **Pen** instead. Click for a corner point, or press and drag to make a smooth curve point. Finish and close it as for a line.

On the phone, tap the points, then tap **Finish** or tap the first point to close. The Pen is under **More > Desktop features** on the phone.

## Draw polygons and stars

To draw a polygon or a star:

1. Click **Polygon**.
2. In the tool bar, set **Sides** (or **Points** for a star), tick **Star** for a star, and set **Inner** (the inner points' radius as a percentage) and **Corner radius**.
3. Drag from the centre to where the first corner goes. Hold `Shift` to turn in 15° steps.

A tap without a drag places a 20 mm shape.

The phone's tool strip leaves out **Polygon**; start it from **More > Desktop features**.

## Common mistakes

- **A key chose a tool instead of aligning, or the reverse.** With one object selected, `R`, `E`, `C` and `T` pick tools; with two or more they align.
- **A letter key did nothing.** Single-letter keys work only while the canvas has the focus. Click the canvas first.
- **The new shape went on the wrong layer.** New shapes go on the current layer. Click the layer in **Cuts / Layers** before you draw, or move the shape afterwards with **Layer** in Properties.
