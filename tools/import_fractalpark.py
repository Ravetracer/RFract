#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
"""Copies FractalPark's published formula library into RFract (bundled formulas).

    git clone --depth 1 https://github.com/noodle-bag/fractalpark _dev/fractalpark
    python3 tools/import_fractalpark.py _dev/fractalpark

Only the *published* set is taken (public/formula-library/v1/runtime/published): FractalPark
released these Definitions under the MIT license after its own rights review. Its unpublished
candidate folders (resources/formula-library/…) and the GPL rows it holds back are not part of
that set and are never copied.

Writes crates/rfr-core/formulas/fractalpark/: definitions/*.frm, index.json (names, families,
parameters, default views), LICENSE (FractalPark's MIT license) and SOURCE.md (repository,
commit, date, counts). Run it again to update; the old copy is replaced.
"""

import argparse
import datetime
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "crates" / "rfr-core" / "formulas" / "fractalpark"
REPO = "https://github.com/noodle-bag/fractalpark"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("checkout", type=Path, help="a clone of the FractalPark repository")
    a = ap.parse_args()
    src = a.checkout / "public" / "formula-library" / "v1" / "runtime" / "published"
    index_path = src / "index.json"
    license_path = a.checkout / "LICENSE"
    if not index_path.is_file() or not license_path.is_file():
        sys.exit(f"{a.checkout} does not look like a FractalPark checkout (no published index or LICENSE)")
    if "MIT License" not in license_path.read_text(encoding="utf-8"):
        sys.exit("FractalPark's LICENSE is no longer the MIT license: check before bundling")

    index = json.loads(index_path.read_text(encoding="utf-8"))
    rows = index["rows"]
    if index.get("rowCount") != len(rows):
        sys.exit(f"index.json: rowCount {index.get('rowCount')} but {len(rows)} rows")

    shutil.rmtree(DEST, ignore_errors=True)
    (DEST / "definitions").mkdir(parents=True)
    for row in rows:
        rel = Path(row["definitionPath"])
        if rel.parent.name != "definitions" or rel.suffix != ".frm" or ".." in rel.parts:
            sys.exit(f"unexpected definition path: {rel}")
        shutil.copy2(src / rel, DEST / "definitions" / rel.name)
    shutil.copy2(index_path, DEST / "index.json")
    shutil.copy2(license_path, DEST / "LICENSE")

    commit = subprocess.run(["git", "-C", str(a.checkout), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip() or "unknown"
    bases = {}
    for row in rows:
        bases[row.get("implementationBasis", "?")] = bases.get(row.get("implementationBasis", "?"), 0) + 1
    basis_lines = "\n".join(f"| {k} | {v} |" for k, v in sorted(bases.items()))
    (DEST / "SOURCE.md").write_text(
        f"""# FractalPark formula library

Bundled from FractalPark ({REPO}), MIT license (see LICENSE in this folder).

| | |
|---|---|
| Commit | `{commit}` |
| Imported | {datetime.date.today().isoformat()} |
| Definitions | {len(rows)} (the published set, decision revision {index.get('decisionRevision')}) |

Implementation basis as recorded by FractalPark:

| Basis | Definitions |
|---|---|
{basis_lines}

Language: FractalPark FRM-like v1 (`frm-like/1`), specified in FractalPark's
docs/specs/frm-like-language-v1.md. Updated with `python3 tools/import_fractalpark.py <checkout>`.
""",
        encoding="utf-8",
    )
    print(f"copied {len(rows)} definitions from commit {commit[:12]} to {DEST.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
