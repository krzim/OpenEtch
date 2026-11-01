# OpenEtch extension for Inkscape

Extensions > OpenEtch > "Save selection to OpenEtch" posts the selected objects (or the whole drawing) as SVG to a
running OpenEtch engine (`POST /api/document/upload?format=svg`), keeping their place and size. The drawing is not
changed and nothing fires: a person still approves runs in OpenEtch.

Install: copy `openetch_send.inx` and `openetch_send.py` to Inkscape's user extensions folder (Edit > Preferences >
System > User extensions) and restart Inkscape. Inkscape 1.x (`inkex` 1.x).

Not tested inside Inkscape yet: `tools/tests/test_integrations.py` covers the selection pruning and the upload
against a stub engine.
