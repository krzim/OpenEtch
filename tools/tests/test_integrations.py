"""Tests of the CAD integrations' plain-Python parts (no Fusion or Inkscape needed): URL building, the upload
request (against a local stub engine) and the Inkscape selection pruning.

    python -I tools/tests/test_integrations.py
"""

import importlib.util
import json
import os
import sys
import threading
import types
import unittest
import xml.etree.ElementTree as ET
from http.server import BaseHTTPRequestHandler, HTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def fake_fusion_api():
    """Just enough of Fusion's `adsk` package for the add-in module to import."""
    adsk, core, fusion_mod = (types.ModuleType(n) for n in ("adsk", "adsk.core", "adsk.fusion"))
    core.CommandEventHandler = object
    core.CommandCreatedEventHandler = object
    adsk.core, adsk.fusion = core, fusion_mod
    sys.modules.update({"adsk": adsk, "adsk.core": core, "adsk.fusion": fusion_mod})


fake_fusion_api()
inkscape = load("openetch_send", os.path.join(HERE, "..", "inkscape-extension", "openetch_send.py"))
fusion = load("openetch_fusion", os.path.join(HERE, "..", "fusion360-addin", "OpenEtch", "OpenEtch.py"))


class Stub(BaseHTTPRequestHandler):
    seen = []

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        Stub.seen.append((self.path, self.headers.get("Authorization"), self.headers.get("Content-Type"), body))
        out = json.dumps({"ok": True, "result": {"created": ["obj-1", "obj-2"]}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *args):
        pass


class Integrations(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = HTTPServer(("127.0.0.1", 0), Stub)
        cls.url = "http://127.0.0.1:{}".format(cls.server.server_address[1])
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def test_urls_encode_the_name(self):
        self.assertEqual(
            fusion.upload_url("http://pi:7878/", "Sketch 1 & co"),
            "http://pi:7878/api/document/upload?format=dxf&mode=merge&name=Sketch+1+%26+co",
        )
        self.assertTrue(inkscape.upload_url("http://x", "a").endswith("?format=svg&mode=merge&name=a"))

    def test_posts_with_the_token(self):
        Stub.seen.clear()
        r = fusion.post_dxf(self.url, "secret", "Plate", b"0\nSECTION\n")
        self.assertTrue(r["ok"])
        r = inkscape.post_svg(self.url, "", "Logo", "<svg/>")
        self.assertEqual(len(r["result"]["created"]), 2)
        (p1, auth1, ct1, b1), (p2, auth2, ct2, b2) = Stub.seen
        self.assertTrue(p1.startswith("/api/document/upload?format=dxf"))
        self.assertEqual((auth1, ct1, b1), ("Bearer secret", "application/dxf", b"0\nSECTION\n"))
        self.assertEqual((auth2, ct2, b2), (None, "image/svg+xml", b"<svg/>"))

    def test_prune_keeps_the_selection_and_its_groups(self):
        svg = ET.fromstring(
            '<svg xmlns="http://www.w3.org/2000/svg"><defs id="d"/>'
            '<g id="layer1" transform="translate(5,0)"><rect id="a"/><rect id="b"/></g>'
            '<g id="layer2"><circle id="c"/></g></svg>'
        )
        inkscape.prune(svg, {"a"})
        ids = [e.get("id") for e in svg.iter() if e.get("id")]
        self.assertEqual(ids, ["d", "layer1", "a"])


if __name__ == "__main__":
    unittest.main()
