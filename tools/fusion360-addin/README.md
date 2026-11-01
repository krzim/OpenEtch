# OpenEtch add-in for Autodesk Fusion

"Send to OpenEtch" exports the selected sketch (or a selected planar face's outline, or the sketch being edited) as
DXF and posts it to a running OpenEtch engine (`POST /api/document/upload?format=dxf`). The drawing arrives in the
open document as one group. Nothing fires: a person still approves runs in OpenEtch.

Install: copy the `OpenEtch` folder to Fusion's add-ins folder (Utilities > Add-Ins > Scripts and Add-Ins > the green
"+" next to My Add-Ins), then Run it. The button is under Utilities > Add-Ins. Engine URL and token (only needed off
loopback; the engine prints it at startup) are asked for and kept in `openetch-settings.json` next to the add-in.

Not tested inside Fusion yet (no Fusion API in CI): `tools/tests/test_integrations.py` covers the request building and
upload against a stub engine.
