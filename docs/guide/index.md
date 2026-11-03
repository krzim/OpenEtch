---
title: OpenEtch user guide
order: 0
section: Getting started
needs: design
summary: What the guide covers, how to open it from the app and how to find a topic.
---

# OpenEtch user guide

OpenEtch designs and runs jobs for galvo fibre lasers with BJJCZ (LMC) control boards. This guide shows you how to use the app on a PC, a MacBook, an iPad or a phone.

The app looks different on each. A PC or a MacBook has a ribbon at the top and docked windows beside the canvas. A phone has four tabs at the bottom (**Canvas**, **Layers**, **Job** and **More**), and each opens a sheet over the canvas. An iPad has a tool rail on the left and panels that slide in from the sides. Where a step differs, the guide says what to do on each.

> **Note:** Many features have only been tested on the simulator, not on every laser. Settings in this guide are starting points to test on your own material, not guaranteed results.

## Open the guide {#open-the-guide}

To open the guide from the app:

- On a PC with a wide window, choose **Help > User guide**, or press `F1`.
- On a MacBook or in a narrower window, the Help menu is inside **More** (the ⋯ button at the top right). Choose **More**, then **User guide** under **Help**. On a MacBook keyboard you may need `fn+F1`.
- On an iPad, tap **More** (⋯) at the top right, then **User guide**.
- On a phone, tap the **More** tab, open **Help**, then tap **User guide**.

You can also use command search: press `S` (on a phone, tap **More**, then **Command search…**) and type "guide".

On a PC, a MacBook or an iPad held sideways, the guide opens beside the canvas, so you can follow the steps while you work. **Open in new tab** shows it on its own in a browser tab. On a phone, or an iPad held upright, the guide fills the screen under the top bar. **Back** returns to the page you read before, and **Close the guide** returns to the app.

![The user guide open beside the canvas on a MacBook, with the two-tone coin example on the canvas](img/guide-beside-canvas.webp)

## Find a topic {#find-a-topic}

- **Contents** lists every page, grouped by section.
- Type in **Search the guide** to search page titles, headings and text.
- A small **?** button next to a panel or a dialog opens the guide at the part that explains it.
- Pages marked **Needs a laser** only matter when a laser is connected.

## Keyboard shortcuts {#keyboard-shortcuts}

To see every keyboard shortcut, press `Shift+?` or choose **Help > Keyboard shortcuts**. The keys are LightBurn's defaults where LightBurn has one.

To change a key:

1. In **Keyboard shortcuts**, choose **Edit…**.
2. Choose **+** next to the action, then press the new key.
3. Choose **Done editing**.

Your keys are saved in this browser only. A browser keeps a few keys for itself (such as `Ctrl+W` and `Ctrl+T`), so those cannot be used.

## Stay safe {#stay-safe}

> **Warning:** A fibre laser can blind you in an instant, and its beam is invisible. Wear eye protection rated for your laser's wavelength (1064 nm for most fibre lasers) whenever it can fire. Use fume extraction. Never leave a job running unattended.

Before you try a new setting or a test grid, work out its energy density. See [Energy density](example.md#energy-density) for how.

## AI agents {#ai-agents}

OpenEtch has an MCP server, so an AI agent can work on your design: draw shapes, change layers and prepare a job. Through these tools an agent can never approve or start a burn. A job it prepares waits as a pending run until a person starts it in the app (with the operator PIN, if the engine uses one). The foot switch never starts an agent's job: it only burns the design it has just framed.

> **Warning:** This protection covers the MCP tools only. An agent that can also control your browser, or run programs on the computer the laser is connected to, can do whatever you can. If you let an agent do that while nobody is at the laser, run the engine with an operator PIN.

## Contents {#contents}

**Setup and reference**

- [Page format example](example.md): Every markdown feature a guide page can use, for checking the renderer. (Needs a laser)
