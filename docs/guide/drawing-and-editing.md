---
title: Drawing and editing
order: 5
section: Designing
needs: design
summary: Where the drawing tools are on each screen, pan and zoom, the Edit menu and curve tolerance, undo and redo with the timeline, and the pages for each kind of edit.
---

# Drawing and editing

This page shows where the drawing and editing tools of the **Design** workspace are, and how to move round the canvas and undo. Every edit you make is one engine command and one step on the timeline, so you can always undo it.

> **Note:** On a Mac, press `Cmd` where this guide says `Ctrl`, and `Option` where it says `Alt`. On a Mac, `Ctrl`+click is the right-click. Menus on a Mac show the keys as symbols (⌘ ⌥ ⇧).

## The drawing pages

The tools themselves each have a page:

- [Drawing shapes](drawing-shapes.md): rectangles, ellipses, circles, lines, curves, polygons and stars, and Shape properties.
- [Selecting and transforming](selecting-and-transforming.md): select, move, rotate, scale, slant and mirror, on the canvas, with the keyboard or by numbers.
- [Snapping and guides](snapping-and-guides.md): snap to objects and the grid, place guides, and measure.
- [Nodes and path editing](nodes-and-paths.md): edit nodes, trim, combine, offset and fillet shapes, tidy paths, and warp or deform.
- [Arranging and arrays](arranging-and-arrays.md): align, distribute, order, group and lock, and grid and circular arrays and nesting.
- [Text](text.md) and [Images](images.md) have their own pages too.

## Where the tools are

The tools sit in different places on each kind of screen.

| Screen | Where the drawing tools are |
|---|---|
| Computer (wide window) | The ribbon at the top, in the groups **Create**, **Modify**, **Arrange**, **Insert**, **Inspect** and **Laser Tools**. In a narrow window, the least-used groups fold into a button with the group's name; click it to open the group. |
| Tablet | A tool rail down the left edge of the canvas. Groups that do not fit open sideways as a flyout. |
| Phone | Tap the **Canvas** tab at the bottom. The sheet that opens holds the tool strip, which scrolls sideways. |

The ribbon groups hold these tools:

| Group | Tools |
|---|---|
| **Create** | **Stock**, **Rectangle**, **Ellipse**, **Circle**, **Line**, **Pen**, **Polygon**, **Text**, **Barcode** |
| **Modify** | **Edit nodes**, **Trim**, the **Modify** menu, **Fill Region**, the **Edit** menu, the **Tools** menu, **Web Tools**, **Delete** |
| **Arrange** | The **Arrange** menu (and a row of align buttons when two or more objects are selected), the **Jigs** menu |
| **Insert** | **Import**, **Place**, **Array**, **Art** |
| **Inspect** | **Measure**, the snap and view toggles, and the result colours toggle |
| **Laser Tools** | Material, interval and focus tests, the Material Library and the MOPA materials (see the laser pages) |

To find any command by name, press `S` or click **Search** in the ribbon, then type. On the phone, use **More > Command search…**

To see a short menu of what you can do with the selection, right-click it. On a touch screen, press and hold (long-press) instead. A long right-click menu files its entries under **Modify**, **Arrange**, **Select** and **Laser** submenus.

> **Tip:** To put the creation tools in a vertical rail on a computer, as LightBurn does, right-click empty space in the ribbon and tick **Creation tools on a vertical rail (LightBurn style)**.

### Tools that are not on the phone

The phone's tool strip and menus leave out **Pen**, **Polygon**, **Edit nodes**, **Trim**, **Weld** and **Add Tabs**, because they are made for a bigger screen and a mouse or a pen. You can still start them from **More > Desktop features**. Docking and editing the Art Library work only on a computer or tablet.

![The Design ribbon on a computer, with the Create, Modify, Arrange, Insert and Inspect groups](img/design-ribbon.webp)

## Pan and zoom

- To zoom, turn the mouse wheel, or press `Ctrl+=` and `Ctrl+-`. `Ctrl+0` zooms to the whole field.
- To pan, drag with the middle mouse button, or hold `Space` and drag.
- On a touch screen, pan with two fingers and pinch to zoom.
- The buttons at the top right of the canvas are **Fit field** and **Zoom to selection**.

On a Mac trackpad, two-finger scrolling pans and a pinch zooms. To change this, use **Trackpad: scroll pans, pinch zooms** in Preferences.

## The Edit menu

The **Edit** menu in the **Modify** group holds:

- **Undo** and **Redo**.
- **Cut** (`Ctrl+X`), **Copy** (`Ctrl+C`), **Paste** (`Ctrl+V`) and **Paste in place** (`Alt+V`).
- **Duplicate** (`Ctrl+D`) and **Delete** (`Delete` or `Backspace`).
- **Select all**, **Invert selection** and **Select** (open or contained shapes).
- **Use the system clipboard**: when on, copy and paste go through your computer's clipboard, so you can paste art or an image (up to 50 MB) from another program.
- **Curve tolerance…**

### Curve tolerance

Curve tolerance sets how finely a job turns curves into straight marks: the largest distance between a curve and the straight pieces that replace it. Coarser marks faster with fewer points; finer follows curves more closely. Enter 0.001 to 1 mm, then click **Save**. It is saved with the project. Leave it empty to use the device's setting (0.01 mm unless the device says otherwise).

## Undo, redo and the timeline

- **Undo**: `Ctrl+Z`, or the **Undo** button in the top bar.
- **Redo**: `Ctrl+Shift+Z` or `Ctrl+Y`, or the **Redo** button.
- On a touch screen, tap with two fingers to undo and with three fingers to redo. The phone also has Undo and Redo buttons at the bottom right of the canvas.

A step you undo is kept until your next edit, which discards it. Until then, Redo brings it back.

### The timeline

The **Timeline** along the bottom shows one chip per step, oldest on the left, like Fusion 360's design timeline. Each chip says what the step did ("Rectangle", "Move", Text "Hello") and who did it: **You**, an AI agent by name, or **Script**. A step made by an agent flashes briefly when it arrives.

To roll the design back:

1. Click a step, or drag the marker to it.
2. The steps after the marker turn grey. They are kept, not deleted.
3. A new edit goes in at the marker, and the later steps are replayed after it. A step that no longer applies is shown red.

Right-click a step (or tap or long-press it on a touch screen) for **Roll marker here**, **Roll marker before this step** and **Delete step**. The buttons **Go to start**, **Previous step**, **Next step** and **Go to end** step through the history.

In the compact and tablet layouts the timeline shares the bottom row with the status bar. In the compact layout, click **Pin the timeline open** to keep it showing. On the phone, the timeline is a list under **More > History**.

> **Note:** AI agents connected over MCP edit the same design, and their steps appear on the timeline like yours. Agents can design, but they can never approve or start a burn. See [AI agents](ai-agents.md).

![The left end of the timeline: New document, Rectangle, Ellipse and a grey Move step after the marker](img/timeline.webp)

## Touch controls

On a tablet or phone, one finger works like the left mouse button, two fingers pan and zoom, and a long-press is the right-click. The modifier bar gives you `Shift` and the other keys a finger lacks, and the selection action bar puts Copy, Duplicate, Rotate 90° and more next to the selection. See [Touch controls](the-workspace.md#touch-controls) in The workspace.

## Keyboard shortcuts

OpenEtch uses LightBurn's default keys. To see them all, press `?` (`Shift+?`). `F1` opens this guide. To change a key, choose **File > Settings > Edit hotkeys…** Your keys are saved in this browser.

## Common mistakes

- **A letter key did nothing.** Single-letter keys work only while the canvas has the focus. Click the canvas first.
- **Redo does nothing.** A step you undo is gone once you make another edit. To go back without losing the later steps, move the timeline marker instead: the steps after it are kept and replayed after your new edit.
- **Paste brought in the wrong thing.** With **Use the system clipboard** on, paste takes what you last copied anywhere on your computer. Turn it off in the **Edit** menu to paste only inside OpenEtch.
