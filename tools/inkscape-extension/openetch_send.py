#!/usr/bin/env python3
"""Inkscape extension: Extensions > OpenEtch > Save selection to OpenEtch.

Copies the selected objects (or the whole drawing when nothing is selected) into a standalone SVG with the
document's size and viewBox, so they keep their place and size, and posts it to a running OpenEtch engine
(POST /api/document/upload?format=svg), which adds it to the open document. The drawing is not changed. Nothing
fires: marking still needs a person to approve the run in OpenEtch.

The token is only needed when the engine listens off loopback (the engine prints it at startup); it is sent as
"Authorization: Bearer <token>". Licence: MIT OR Apache-2.0 (part of OpenEtch).
"""

import copy
import json
import urllib.error
import urllib.parse
import urllib.request

try:
    import inkex
except ImportError:  # Allows the helpers below to be tested without Inkscape.
    inkex = None


def upload_url(engine, name):
    query = urllib.parse.urlencode({"format": "svg", "mode": "merge", "name": name})
    return engine.rstrip("/") + "/api/document/upload?" + query


def post_svg(engine, token, name, svg_text, timeout=30):
    req = urllib.request.Request(upload_url(engine, name), data=svg_text.encode("utf-8"), method="POST")
    req.add_header("Content-Type", "image/svg+xml")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


NON_DRAWING = ("defs", "namedview", "metadata", "title", "desc", "style")


def local_name(el):
    return el.tag.split("}")[-1] if isinstance(el.tag, str) else ""


def prune(root, keep_ids):
    """Remove from `root` every drawing element that is not one of `keep_ids`, inside one, or an ancestor of one
    (groups keep their transforms). Definitions and metadata stay. Works on any ElementTree-like tree."""

    def holds_kept(el):
        return any(e.get("id") in keep_ids for e in el.iter())

    def walk(parent):
        for child in list(parent):
            if local_name(child) in NON_DRAWING or child.get("id") in keep_ids:
                continue
            if holds_kept(child):
                walk(child)
            else:
                parent.remove(child)

    walk(root)
    return root


if inkex is not None:

    class SaveToOpenEtch(inkex.EffectExtension):
        def add_arguments(self, pars):
            pars.add_argument("--engine", default="http://127.0.0.1:7878")
            pars.add_argument("--token", default="")
            pars.add_argument("--name", default="Inkscape selection")

        def selection_svg(self):
            """A copy of the document with only the selected elements (with their ancestors' transforms)."""
            svg = self.svg
            doc = copy.deepcopy(svg)
            selected = list(self.svg.selected.values()) if len(self.svg.selected) else None
            if selected is None:
                return doc.tostring().decode("utf-8")
            prune(doc, {el.get("id") for el in selected})
            return doc.tostring().decode("utf-8")

        def effect(self):
            text = self.selection_svg()
            try:
                answer = post_svg(self.options.engine, self.options.token, self.options.name, text)
            except urllib.error.HTTPError as e:
                inkex.errormsg("OpenEtch answered {}: {}".format(e.code, e.read().decode("utf-8", "replace")))
                return
            except urllib.error.URLError as e:
                inkex.errormsg("Cannot reach OpenEtch ({}). Is the engine running?".format(e.reason))
                return
            if not answer.get("ok"):
                inkex.errormsg("OpenEtch refused the drawing: {}".format(answer.get("error")))
                return
            created = answer.get("result", {}).get("created", [])
            inkex.errormsg("Sent to OpenEtch: {} objects.".format(len(created)))

    if __name__ == "__main__":
        SaveToOpenEtch().run()
