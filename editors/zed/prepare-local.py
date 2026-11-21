#!/usr/bin/env python3
"""Stage an installable Zed developer extension from this Git checkout."""

import json
import shutil
import subprocess
from pathlib import Path

extension = Path(__file__).resolve().parent
repo = extension.parents[1]
grammar = "editors/tree-sitter-oxel"
target = repo / "target" / "zed-oxel"

# Zed checks out the parser from Git, even when installing a developer extension.
revision = subprocess.check_output(
    ["git", "log", "-1", "--format=%H", "--", grammar], cwd=repo, text=True
).strip()
if not revision:
    raise SystemExit("The Tree-sitter grammar needs a commit before Zed can fetch it.")

manifest = (extension / "extension.toml").read_text(encoding="utf-8")
header, grammar_section = manifest.split("[grammars.oxel]\n", 1)
_, remainder = grammar_section.split("\n[", 1)
local_grammar = (
    "[grammars.oxel]\n"
    f"repository = {json.dumps(repo.as_uri())}\n"
    f"rev = {json.dumps(revision)}\n"
    f"path = {json.dumps(grammar)}\n\n"
)
target.mkdir(parents=True, exist_ok=True)
(target / "extension.toml").write_text(header + local_grammar + "[" + remainder, encoding="utf-8")
for name in ("Cargo.toml", "Cargo.lock"):
    shutil.copy2(extension / name, target / name)
for name in ("languages", "src"):
    shutil.copytree(extension / name, target / name, dirs_exist_ok=True)
print(target)
