---
title: Design in your browser
order: 4.5
section: Getting started
needs: design
summary: OpenEtch Design runs the editor in a web page with no engine and no laser - what it can do, where it keeps your projects, and how to move work to the full app or to LightBurn.
---

# Design in your browser

**OpenEtch Design** is the OpenEtch editor as a web page. It runs entirely in your browser: there is no engine to install, nothing is uploaded, and it never talks to a laser. Use it to draw, prepare and preview designs on any PC, Mac, iPad or phone, then burn them with the full OpenEtch app or with LightBurn.

You can tell you are in OpenEtch Design by the strip under the top bar: **OpenEtch Design: runs in your browser, nothing is uploaded**. **Get the full app** in that strip, and in the **Help** menu, opens the project's page with the full app.

## What it can do

Everything in the **Designing** and **Layers and materials** sections of this guide works here:

- drawing, text (in the bundled fonts or fonts you add), images, tracing, Fill Region, fit to stock, arrays and templates;
- layers and cut settings, material libraries, MOPA colour and test grids;
- importing SVG, DXF, PDF, AI, images and LightBurn `.lbrn2` files;
- exporting SVG, DXF, PDF, AI and LightBurn `.lbrn2`, and printing.

The **Laser** workspace is called **Preview** here. It shows the toolpath and the time it would take, labelled **Estimate for** the design device you pick. Pick the device whose field size matches your laser: the built-in ComMarker B6 MOPA, a generic galvo with a 110, 150, 200 or 300 mm field, or a device imported from a LightBurn `.lbdev` file or `prefs.ini` (**File > Laser > Devices**, **Import File**).

What it leaves out is everything that needs a machine: framing, starting a burn, the job controls, the camera, the production cockpit and the machine settings. The pages about those are not in this copy of the guide.

## Where your projects are kept

**File > Save** keeps the project in this browser, on this device. **File > Open** lists them under **Saved in this browser**. Autosave and the backups made when you save over a project work as they do in the full app, inside the browser too.

The browser can lose that storage, so keep copies of anything that matters:

- **File > Download a copy** saves the project as an `.openetch` file on your device. Open it again with **File > Open**, **Open a file from this device**.
- Clearing the browser's site data deletes the projects.
- Safari deletes a site's data after 7 days without a visit, unless you add the site to the Home Screen or the Dock.
- In a private window nothing is kept once the window closes. OpenEtch Design says so when it starts.

If you reload the page, or the editor stops after an error and starts again, your unsaved design comes back, and a message says from what time.

You can open OpenEtch Design in two tabs or windows. Each has its own document, and both see the same saved projects. If you save a project that the other tab saved after you opened it, OpenEtch Design asks before replacing that version.

## Fonts

The bundled single-line fonts are always there. To use one of your own fonts, add its `.ttf` or `.otf` file in the font picker. It is kept in the browser with your projects, so it is there next time. A LightBurn file that names a font you have not added shows **Missing font** on that text, as in the full app.

## Moving work to the full app or to LightBurn

- **To the full app:** **File > Download a copy**, then in the full app **File > Open** that `.openetch` file, or import it.
- **To LightBurn:** **File > Export LightBurn .lbrn2**, then open the file in LightBurn. Layers keep their cut settings.
- **From LightBurn:** open or import the `.lbrn2` file here.

OpenEtch is not affiliated with LightBurn Software.

## Common mistakes

- **Only saving in the browser.** Saved projects live in this browser on this device only. Download a copy of anything you want to keep or move.
- **Trusting the time estimate for another laser.** The estimate is for the design device shown. Pick the one with your laser's field size, or import your LightBurn device.
- **Expecting a burn button.** OpenEtch Design has no laser control. Burn with the full OpenEtch app or with LightBurn.
