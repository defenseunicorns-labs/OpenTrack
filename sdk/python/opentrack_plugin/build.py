"""Build a plugin module into a WebAssembly component with componentize-py.

The module must be plain Python (the standard library and pure-Python
packages): C extensions such as numpy cannot run inside the component.
Plugins that need them run as external plugins instead (`serve`).
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def wit_dir() -> Path:
    """The plugin interface: bundled with the SDK, or the repository's."""
    for candidate in (HERE / "wit", HERE.parent.parent.parent / "wit"):
        if (candidate / "plugin.wit").exists():
            return candidate
    raise SystemExit("wit/plugin.wit not found: build from an OpenTrack checkout")


def build(module_file: str, output: str) -> None:
    exe = shutil.which("componentize-py") or str(Path(sys.executable).with_name("componentize-py"))
    if not Path(exe).exists():
        raise SystemExit("componentize-py is not installed: pip install 'opentrack-plugin[component]'")
    module = Path(module_file).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        app = Path(tmp) / "opentrack_app.py"
        app.write_text(
            f"import {module.stem} as plugin_module\n"
            "from opentrack_plugin.component import export\n"
            "Meta, Codec, Tracker, Scorer = export(plugin_module.PLUGIN)\n"
        )
        cmd = [
            exe, "-d", str(wit_dir()), "-w", "plugin", "componentize", "opentrack_app",
            "-p", tmp, "-p", str(module.parent), "-p", str(HERE.parent),
            "-o", str(Path(output).resolve()),
        ]
        subprocess.run(cmd, check=True, env=dict(os.environ))
    print(f"built {output}")
