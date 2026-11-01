"""OpenEtch add-in for Autodesk Fusion: "Send to OpenEtch".

Exports the selected sketch (or the profile of a selected planar face, or else the active sketch) to DXF and posts it
to a running OpenEtch engine (POST /api/document/upload?format=dxf), which adds it to the open document as one
group, as LightBurn imports a DXF. Nothing fires: marking still needs a person to approve the run in OpenEtch.

Settings live in openetch-settings.json next to this file (created on first use):

    {"engine": "http://127.0.0.1:7878", "token": ""}

The token is only needed when the engine listens off loopback (it prints it at startup). It is sent as
"Authorization: Bearer <token>" and never logged. Licence: MIT (part of OpenEtch).
"""

import json
import os
import tempfile
import traceback
import urllib.error
import urllib.parse
import urllib.request

import adsk.core
import adsk.fusion

CMD_ID = "openetchSendToEngine"
CMD_NAME = "Send to OpenEtch"
PANEL_ID = "SolidScriptsAddinsPanel"
SETTINGS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "openetch-settings.json")

_handlers = []


def load_settings():
    try:
        with open(SETTINGS, "r", encoding="utf-8") as f:
            s = json.load(f)
    except (OSError, ValueError):
        s = {}
    s.setdefault("engine", "http://127.0.0.1:7878")
    s.setdefault("token", "")
    return s


def save_settings(s):
    with open(SETTINGS, "w", encoding="utf-8") as f:
        json.dump({"engine": s["engine"], "token": s["token"]}, f, indent=2)


def upload_url(engine, name):
    """The upload endpoint for a DXF named `name` (query values URL-encoded)."""
    query = urllib.parse.urlencode({"format": "dxf", "mode": "merge", "name": name})
    return engine.rstrip("/") + "/api/document/upload?" + query


def post_dxf(engine, token, name, data):
    """POST `data` (DXF bytes) to the engine; returns the decoded JSON answer."""
    req = urllib.request.Request(upload_url(engine, name), data=data, method="POST")
    req.add_header("Content-Type", "application/dxf")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read().decode("utf-8"))


def sketch_to_dxf(sketch):
    """Save `sketch` as DXF to a temporary file and return its bytes."""
    fd, path = tempfile.mkstemp(suffix=".dxf")
    os.close(fd)
    try:
        if not sketch.saveAsDXF(path):
            raise RuntimeError("Fusion could not save the sketch as DXF")
        with open(path, "rb") as f:
            return f.read()
    finally:
        try:
            os.remove(path)
        except OSError:
            pass


def face_to_dxf(face):
    """Project a planar face's edges into a temporary sketch on it, save that as DXF, delete the sketch."""
    comp = face.body.parentComponent
    sketch = comp.sketches.add(face)
    try:
        sketch.project(face)
        return sketch_to_dxf(sketch)
    finally:
        sketch.deleteMe()


def selected_geometry(app):
    """(name, dxf bytes) for the selection: a sketch, a planar face, or the active sketch."""
    ui = app.userInterface
    design = adsk.fusion.Design.cast(app.activeProduct)
    if design is None:
        raise RuntimeError("open a design first")
    for i in range(ui.activeSelections.count):
        e = ui.activeSelections.item(i).entity
        sketch = adsk.fusion.Sketch.cast(e)
        if sketch:
            return sketch.name, sketch_to_dxf(sketch)
        face = adsk.fusion.BRepFace.cast(e)
        if face and face.geometry.surfaceType == adsk.core.SurfaceTypes.PlaneSurfaceType:
            return face.body.name + " face", face_to_dxf(face)
    active = adsk.fusion.Sketch.cast(design.activeEditObject)
    if active:
        return active.name, sketch_to_dxf(active)
    raise RuntimeError("select a sketch or a planar face (or edit a sketch) first")


class ExecuteHandler(adsk.core.CommandEventHandler):
    def notify(self, args):
        app = adsk.core.Application.get()
        ui = app.userInterface
        try:
            inputs = args.command.commandInputs
            s = load_settings()
            s["engine"] = inputs.itemById("engine").value.strip() or s["engine"]
            s["token"] = inputs.itemById("token").value.strip()
            save_settings(s)
            name, data = selected_geometry(app)
            answer = post_dxf(s["engine"], s["token"], name, data)
            if answer.get("ok"):
                created = answer.get("result", {}).get("created", [])
                ui.messageBox("Sent \"{}\" to OpenEtch ({} objects).".format(name, len(created)), CMD_NAME)
            else:
                ui.messageBox("OpenEtch refused the drawing: {}".format(answer.get("error")), CMD_NAME)
        except urllib.error.HTTPError as e:
            ui.messageBox("OpenEtch answered {}: {}".format(e.code, e.read().decode("utf-8", "replace")), CMD_NAME)
        except urllib.error.URLError as e:
            ui.messageBox("Cannot reach OpenEtch ({}). Is the engine running?".format(e.reason), CMD_NAME)
        except Exception as e:  # noqa: BLE001 - shown to the user
            ui.messageBox("{}\n\n{}".format(e, traceback.format_exc()), CMD_NAME)


class CreatedHandler(adsk.core.CommandCreatedEventHandler):
    def notify(self, args):
        s = load_settings()
        cmd = args.command
        cmd.commandInputs.addStringValueInput("engine", "Engine", s["engine"])
        cmd.commandInputs.addStringValueInput("token", "Token (off loopback)", s["token"])
        on_execute = ExecuteHandler()
        cmd.execute.add(on_execute)
        _handlers.append(on_execute)


def run(context):
    ui = adsk.core.Application.get().userInterface
    try:
        cmd_def = ui.commandDefinitions.itemById(CMD_ID) or ui.commandDefinitions.addButtonDefinition(
            CMD_ID, CMD_NAME, "Send the selected sketch or planar face to OpenEtch as DXF"
        )
        on_created = CreatedHandler()
        cmd_def.commandCreated.add(on_created)
        _handlers.append(on_created)
        panel = ui.allToolbarPanels.itemById(PANEL_ID)
        if panel and not panel.controls.itemById(CMD_ID):
            panel.controls.addCommand(cmd_def)
    except Exception:  # noqa: BLE001
        ui.messageBox("OpenEtch add-in failed to start:\n{}".format(traceback.format_exc()))


def stop(context):
    ui = adsk.core.Application.get().userInterface
    panel = ui.allToolbarPanels.itemById(PANEL_ID)
    if panel:
        control = panel.controls.itemById(CMD_ID)
        if control:
            control.deleteMe()
    cmd_def = ui.commandDefinitions.itemById(CMD_ID)
    if cmd_def:
        cmd_def.deleteMe()
