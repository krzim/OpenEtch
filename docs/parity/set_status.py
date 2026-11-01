"""Update one or more parity tracker items in parity-data.js.

    python -I docs/parity/set_status.py LAY-067 done "crates/commands/src/builtin/sub_layer.rs; PropertiesPanel pass list" ["note"]

Arguments repeat in groups of 3 or 4 (id, status, evidence, optional note) separated by "--".
Status is one of done, partial, missing, na. The file keeps one item per line.
"""

import json
import sys
from pathlib import Path

DATA = Path(__file__).with_name("parity-data.js")
STATUSES = {"done", "partial", "missing", "na"}


def groups(args):
    group = []
    for a in args:
        if a == "--":
            if group:
                yield group
            group = []
        else:
            group.append(a)
    if group:
        yield group


def main(args):
    lines = DATA.read_text(encoding="utf-8").splitlines()
    index = {}
    for n, line in enumerate(lines):
        s = line.strip()
        if s.startswith('{"id"'):
            index[json.loads(s.rstrip(","))["id"]] = n
    for g in groups(args):
        if len(g) not in (3, 4) or g[1] not in STATUSES:
            sys.exit(f"bad group {g}: expected ID STATUS EVIDENCE [NOTE], status one of {sorted(STATUSES)}")
        item_id, status, evidence = g[:3]
        if item_id not in index:
            sys.exit(f"unknown id {item_id}")
        n = index[item_id]
        item = json.loads(lines[n].strip().rstrip(","))
        item["status"], item["openetch"] = status, evidence
        if len(g) == 4:
            item["notes"] = g[3]
        lines[n] = "    " + json.dumps(item, ensure_ascii=False) + ","
        print(f"{item_id}: {status}")
    DATA.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main(sys.argv[1:])
