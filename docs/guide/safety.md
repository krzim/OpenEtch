---
title: Laser safety
order: 18
section: Running jobs
needs: machine
summary: Eye protection, fume extraction, energy density checks, the STOP button, the safety checks OpenEtch makes before a burn, and what AI agents can and cannot do.
---

# Laser safety

A fibre laser can blind you and start a fire in a fraction of a second. OpenEtch adds checks around every burn, but it cannot make the machine safe on its own. Read this page before your first job.

> **Warning:** OpenEtch is new software. Many features have only run on the simulator so far. Treat every setting and every number in this guide as a starting point to test on scrap, not as a guarantee.

## Before every job

Go through this list each time, even for a repeat job:

1. Put on laser safety eyewear rated for your laser's wavelength.
2. Close the enclosure or fit the guards your machine has.
3. Switch on the fume extractor.
4. Check focus and material height.
5. Check the energy density of every layer you have changed (see [Work out the energy density first](#work-out-the-energy-density-first)).
6. Preview the job and frame it with the red pointer (see [Preview and framing](preview-and-framing.md)).
7. Know where the STOP button and your machine's hardware emergency stop are.
8. Stay with the machine until the job has finished.

You can make OpenEtch ask you to tick a list like this before every burn with the Job Checklist (see [Job Checklist and Require framing](running-a-job.md#job-checklist-and-require-framing)).

## Eye protection

The marking beam of a fibre laser is infrared (around 1064 nm). You cannot see it, and a reflection from shiny metal can damage your eyes before you blink.

- Wear eyewear made for your laser's wavelength, with the optical density the laser maker recommends. Sunglasses do not protect you, and eyewear made for CO2 or diode lasers usually does not protect against 1064 nm.
- Everyone in the room needs eyewear while the laser can fire.
- The red pointer used for framing is a separate, low-power visible light. Seeing it does not mean the marking beam is safe to look at.

## Fume extraction

Marking metal, plastic, coatings and anodising makes fumes and fine dust that are harmful to breathe. Some plastics give off corrosive or toxic gases.

- Run a fume extractor or exhaust for every job, and keep it running for a while after the job ends.
- Never mark PVC or other materials that contain chlorine.
- OpenEtch can switch the extractor for you, keep it on after a job and refuse to start until it is running. See [Extractor](extractor.md).

## Never leave a job unattended

Stay with the machine for the whole job. A fire, or a part that shifts, needs a person there to press STOP.

This matters most for long and repeating work:

- **Repeat** and **Run continuously** keep marking with no further approval (see [Run options](running-a-job.md#run-options)).
- A job that pauses at a layer for you waits up to an hour (see [Pause at a layer](running-a-job.md#pause-at-a-layer)).
- The foot switch loop starts framing again after every burn (see [The foot switch](running-a-job.md#the-foot-switch-frame-then-burn)).
- Notifications to your phone (see [Notifications](notifications.md)) tell you a job has finished or is waiting. They are not a reason to walk away while it burns.

## Work out the energy density first

Before you burn a new setting or a test grid, work out how much energy each setting puts into the surface. This is the energy density, in joules per square millimetre (J/mm²). Too much melts or warps the work, and much too much can start a fire.

OpenEtch does not calculate this for you yet, so do it by hand:

1. Work out the power in watts: the layer's power % times your laser's rated power. On a 60 W laser, 50 % is 30 W.
2. Note the layer's speed in mm/s.
3. Note the line interval (the gap between hatch lines) in mm.
4. Divide: **energy density = power (W) ÷ (speed (mm/s) × line interval (mm))**.

For example, 30 W at 1000 mm/s with a 0.01 mm interval gives 30 ÷ (1000 × 0.01) = 3 J/mm².

| Power | Speed | Interval | Energy density | What to expect on stainless steel (60 W MOPA) |
|---|---|---|---|---|
| 60 W (100 %) | 2000 mm/s | 0.02 mm | 1.5 J/mm² | Light; may barely mark |
| 30 W (50 %) | 1000 mm/s | 0.01 mm | 3 J/mm² | In the usual marking range |
| 30 W (50 %) | 300 mm/s | 0.01 mm | 10 J/mm² | Heavy end of the range |
| 48 W (80 %) | 500 mm/s | 0.005 mm | 19.2 J/mm² | Too high: can melt or warp thin stock |

As a rough guide, on stainless steel with a 60 W MOPA laser, about 2 to 15 J/mm² marks without melting. Higher values can melt or warp thin stock. Other materials need other ranges, so start low and work up.

Things that change the result:

- **Passes add up.** Two passes put in about twice the energy of one.
- **The rated power is an upper limit.** A MOPA laser at short pulse widths or some frequencies delivers less than its rated power, so the real value is usually lower than your sum. Treat your sum as the worst case.
- **Line layers** have no hatch interval. As a rough guide, use the width of the line the beam leaves in place of the interval. The result is often high, so be careful with slow line settings.
- **Colour on steel** comes from heat, so small changes make big differences. See [MOPA colour](mopa-colour.md).

For a test grid, check the corner with the highest power, the lowest speed and the smallest interval before you burn anything. See [Test grids](test-grids.md).

> **Tip:** The **Shade by energy** option in the **Toolpath** card draws the passes that put in more energy darker. It only compares passes with each other; it does not show J/mm². See [The Toolpath card](preview-and-framing.md#the-toolpath-card).

![The right end of the top bar with the status chip showing Framing, its Stop frame button, and the red STOP button](img/stop-button-topbar.webp)

## The STOP button

**STOP** ends any job or frame at once, ends the foot switch loop, and cancels every run that is waiting for approval. It is always allowed: nothing in OpenEtch, not even a pause at a layer or the production cockpit's lock, can hold it up.

Where to find it:

| Layout | Where STOP is |
|---|---|
| Desktop and laptop | At the right of the top bar, just right of the machine status chip. It is never hidden in a menu. The **Laser** window also has a **STOP** button next to **Start**. |
| Tablet | At the right of the top bar. |
| Phone | At the right of the top bar, next to the status dot. It stays in view whatever sheet is open. |

Keyboard shortcuts:

- `Ctrl+Pause` (some keyboards send `Ctrl+Cancel`) stops at any time.
- `Ctrl+.` (`Cmd+.` on a Mac) stops while a run is under way. At other times it keeps its usual meaning (it rotates the selection).
- `Esc` stops framing only. It never stops a burn. It does nothing while you are typing in a field or a menu or dialog is open.

After a stop, a message says what happened: **Stop sent.**, followed by how many runs were cancelled before they started, if any. With no laser connected it says **No laser connected; nothing to stop.**

> **Warning:** STOP is a software stop. It needs the engine to be reachable. Your machine's hardware emergency stop or key switch is the real guarantee. Make sure you can reach it at all times.

### When the connection is lost

If the app loses touch with the OpenEtch engine, a banner appears at the top: **Lost connection to the laser engine – reconnecting…** with **STOP may not reach the laser; use the hardware E-stop.** If the app has not reached the engine at all since it opened, the banner says **Cannot reach the laser engine – retrying…** instead.

While you see this banner, assume STOP in the app will not work. Use the hardware emergency stop if anything is wrong.

## Safety checks before a burn

Every burn has to be approved by a person in the OpenEtch app (or with the foot switch). OpenEtch checks these things each time, whichever way the burn was started:

| Check | What it does | Where to set it |
|---|---|---|
| Approval | A person approves with the operator PIN, or presses **Start** in the app or the foot switch. See [How a burn is approved](running-a-job.md#how-a-burn-is-approved). | When the engine is started |
| **Job Checklist** | Every item must be ticked before the burn. | Device settings, **Job** page |
| **Require framing before start** | The job must have been framed on this device since its last edit. | Device settings, **Job** page |
| **Door Protect input** | No run is approved while the enclosure is open, and a running job aborts if it opens. The door is read again before each part of a job that pauses at a layer. | Device settings, **Ports and laser** page |
| **Stop marking input** | An external button that aborts a running job. | Device settings, **Ports and laser** page |
| Extractor check | Start can be refused until the extractor runs. | Preferences, see [Extractor](extractor.md) |
| Keep-out zones | A job that would mark inside a keep-out zone is refused. | See [Keep-out zones](preflight-and-keep-out.md#keep-out-zones) |
| **Stop at** | No run is approved once the job counter reaches it. | The **Laser** window, see [Job counter](running-a-job.md#job-counter-and-stop-at) |

To open the device settings on a computer or tablet: **File > Laser > Devices…**, select the device, then **Edit…** The **Job** page can only be changed for the active device. On a phone, open **More > Desktop features** and tap **Devices, device settings and the lens wizard…** See [Devices and calibration](devices-and-calibration.md).

## What AI agents can and cannot do

OpenEtch has an MCP server so AI agents can help you design. An agent connected over MCP can **never** approve or start a burn. The engine checks this itself: the approval and Start endpoints refuse any request from an MCP client, the MCP server has no way to arm the laser, and agents never see or handle the operator PIN.

| An agent over MCP can | An agent over MCP cannot |
|---|---|
| Read and edit the design with the same commands the app uses, including a jig's pockets and keep-out zones | Approve a run or press Start |
| Frame the job with the red pointer (light only, galvo lasers) and stop framing | Resume a paused job or **Continue** at a layer pause |
| Create a pending run that waits for you (`job_request_run`), including a re-mark of a region or of one serial slot | See, set or use the operator PIN |
| Press STOP (`machine_stop`) | Change the device's keep-out zones, the Job Checklist, **Require framing before start** or the job log storage settings |
| Read the machine status, the job log and the serial ledger, mark a logged part OK or scrap, and open a burned job | Edit the design while the production cockpit has it locked, or unlock it |

A run an agent prepares shows under **Waiting for approval** in the **Laser** window with the agent's name on it. Read what it will do before you approve it.

> **Warning:** Because a jig's keep-out zones are part of the design, an agent can add, switch off or remove them. Zones saved with the device can only be changed in the app. Check the hatched zones on the canvas before you approve a run an agent prepared.

> **Note:** These limits are for MCP. If you let an agent control a web browser or your desktop, it could press buttons in the app like a person. A program on the engine's computer that is not using MCP could also imitate the app's Start request when the engine runs without a PIN. If that matters to you, run the engine with the operator PIN (see [How a burn is approved](running-a-job.md#how-a-burn-is-approved)) and never tell an agent the PIN. See [AI agents](ai-agents.md).

## Simulator and status badges

- **Ready (simulator)** in the **Laser** window, or **Idle (simulator)** on the status chip, means the engine drives a simulated laser. Nothing fires.
- **SIMULATOR TEST PIN** means the engine was started with a test PIN for the simulator. It is refused on a real laser.
- **ENGINE SHUTTING DOWN** means the engine is stopping or restarting. Nothing new starts until it is back; a running job is left to finish, and STOP still works.
- **ENGINE ABORTING JOB** means the engine is shutting down and is aborting the running job.
