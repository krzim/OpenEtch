---
title: AI agents
order: 33
section: Setup and reference
needs: design
summary: Connect an AI agent to OpenEtch over MCP, what tools it gets, why it has no way to approve or start a burn, and how to ask it for good work.
---

# AI agents

OpenEtch has an MCP server, so an AI agent (an AI assistant that can use tools) can design with you. The agent works on the same live document you see. Its edits appear in the timeline like yours, and you can undo them.

An agent can design, preview, frame with the red pointer and prepare a run. **MCP gives it no way to approve or start a burn.** A person does that, in the app or with the foot switch.

## What an agent can and cannot do

An agent **can**:

- run document commands: draw, import, place, set layers and cut settings, fill templates;
- look at the result as a preview image and read exact numbers;
- undo, redo and move through the timeline;
- list, open and save projects and export LightBurn files;
- switch between open documents (you see the switch);
- frame the job with the red pointer (on a galvo), and stop framing;
- create a **pending** run, which waits for a person;
- press Stop;
- read the machine status, the devices and the production cockpit;
- search the job ledger, open a burned job as a new document, and record a verdict when you tell it how a part came out;
- read the material libraries, edit a preset's finish data, suggest hue nudges and make bracket test grids (library changes stay unsaved until you save the library).

An agent **cannot**:

- approve or start a run, or fire the marking laser in any way;
- see, ask for, guess or set the operator PIN;
- continue a paused run;
- switch, import, edit or remove devices, or change device settings;
- frame a gantry laser (that moves its head);
- edit the design while the production cockpit is open.

## Find the MCP address

The address is shown in **Help > About OpenEtch**, next to **MCP server**. It is the address you opened the app at, followed by `/mcp`, for example `http://127.0.0.1:7878/mcp`.

- On a wide desktop window, **Help** is in the top bar.
- In smaller windows, on MacBooks and on tablets, it is in the **More** (⋯) menu at the right of the top bar.
- On a phone, tap **More**, open **Help** and tap **About OpenEtch**.

## Connect an agent

OpenEtch's MCP server uses the Streamable HTTP transport. Add it to your agent as an HTTP MCP server with the address above. Each agent has its own way to add servers; look for "add an MCP server" or "connectors" in its settings.

### On the same computer

When the agent runs on the engine's computer and the engine listens on `127.0.0.1` (the default), no token is needed. Give the agent the address, for example `http://127.0.0.1:7878/mcp`.

### From another computer

When the engine listens on the network (for example on a Raspberry Pi), every client needs the access token.

1. On the engine's computer (on the Pi, connect with ssh first), run `openetch token`. It prints the token.
2. In the agent's MCP settings, add a header `Authorization: Bearer <token>`.
3. Use the address from **Help > About OpenEtch**.

The agent must send the token as a header; the browser's login cookie does not work for MCP. If the engine is reached by a host name such as `openetch.local`, it must have been started with `--allow-host openetch.local`.

An engine started with `--no-token` needs no token, even on the network. Then anyone who can reach the port can use the MCP endpoint to edit, frame and request runs. Burns still need a person.

## The tools an agent gets

| Area | Tools |
|---|---|
| Commands | `list_commands`, `execute`, `batch` (several commands as one undo step) |
| Looking | `inspect_document`, `render_preview` |
| History | `undo`, `redo`, `history_list`, `history_goto` |
| Machine | `machine_status`, `frame_start`, `frame_stop`, `job_request_run`, `machine_stop` |
| Projects | `project_list`, `project_save`, `project_open`, `project_export_lbrn2` |
| Documents | `documents_list`, `document_activate` |
| Devices | `devices_list`, `lightburn_devices` (read only) |
| Production | `production_status`, `production_events`, `serial_ledger`, `hour_meter`, `serial_remark_request` (pending) |
| Job ledger | `job_ledger_search`, `job_ledger_get`, `job_ledger_mark`, `job_ledger_open` |
| Materials | `material_library`, `material_palette`, `material_finish_update`, `material_hue_nudge`, `material_bracket_test`, `material_bracket_pick`, `material_color_separate` |

`execute` and `batch` run the engine's document commands, the same ones the app uses. None of them moves the machine or fires the laser.

To see the same command list yourself, use **Help > Developer: engine commands (JSON)…**

## Burns always need a person

When an agent calls `job_request_run`, OpenEtch plans the job and creates a pending run. Nothing is sent to the laser.

The pending run shows in the Laser window with a badge carrying the agent's name; hover over it to see **Requested by an AI agent**. To burn it, a person must approve it:

- with the operator PIN (the default): click **Approve…**, type the PIN (6 to 12 digits) and click **Confirm and fire**;
- on an engine started with `--approval click` (no PIN): click **Start** in the OpenEtch app in a browser.

The **Machine** card's **Approval** row shows which of the two your engine uses.

If the engine was started with `--confirm-input`, the run then also waits up to 30 seconds for a press of that physical input after the approval.

The foot switch on its own never starts a run an agent requested. On an engine started with `--approval click`, its first press frames the person's current document, and the next press burns exactly what was framed. Pending runs are started in the app.

If the document changes after the run was requested, approval is refused ("Document changed - request a new run."), so you only burn what you checked. STOP and the hardware E-stop always work; `Ctrl+.` stops while a run is under way.

## Seeing what the agent did

- Agent edits appear in the timeline with their author, like your own steps. Undo or roll back any of them.
- Pending runs from an agent carry the agent's badge.
- When the agent switches the active document, you see it switch in the app.
- Use Preview and frame the job before you approve it.

## Good prompts

Tell the agent what you are making, on what, and what to leave to you. Some examples:

- "Open the example two-tone-coin with doc.new and tell me what each layer does."
- "Open the serial-tags example, fill in serial numbers 101 to 120, run the contact sheet and tell me if any text overflows."
- "Draw a 40 mm circle in the centre with the text 'TEST' inside, 6 mm high, on a Line layer. Render a preview and show me."
- "Fit the design to a 30 mm round stock with a 2 mm margin, then frame it so I can check placement."
- "Suggest settings for a black mark on stainless. Work out the energy density in J/mm² for each setting and keep it between 2 and 15. Do not request a run."
- "Prepare a pending run of the selected shapes only. I will approve it myself."

Tips:

- Say the stock's size and where it sits in the field. Everything is in millimetres from the centre of the field.
- Ask for `render_preview` and `inspect_document` before anything is framed.
- For LightBurn files, tell the agent the field size the file was made for if it is not your active device's.
- Ask the agent to say what it changed. You can always undo.

> **Warning:** Check any settings an agent suggests before you burn, including every box of a bracket test grid. Work out the energy density yourself: J/mm² = power W ÷ (speed mm/s × line interval mm). On stainless with a 60 W MOPA, roughly 2 to 15 J/mm² marked without melting on the reference machine (yours may differ); more can melt or warp thin stock. Wear eye protection rated for your laser's wavelength, run the fume extraction and never leave a job unattended.

## What these limits do not cover

The MCP tools have no command that fires the laser. That does not cover a program that runs on the engine's own computer with your user rights, such as an agent with a shell or one that controls a browser. Such a program could act like the app.

- Use the operator PIN mode (the default) when agents with shell or browser control run on the laser's computer and nobody is watching.
- On an engine without a PIN, any program on the engine's computer can press Start the way the app does. Prefer the PIN mode there.
- OpenEtch is not a safety system. Rely on your machine's hardware E-stop and key switch.

## Troubleshooting agents

- **The agent cannot connect:** check the address in **Help > About OpenEtch**, the token header, and that the engine is reachable from the agent's computer (see [Troubleshooting](troubleshooting.md#cannot-connect-to-the-engine)).
- **401 or "unauthorised":** the token is missing or old. Run `openetch token` again; after `openetch token --rotate` and an engine restart, every client needs the new one.
- **Edits refused while production runs:** the production cockpit locks the design. Close it or wait.
- **A burn did not start:** that is by design. Approve the pending run in the Laser window.
