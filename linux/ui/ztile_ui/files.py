"""Small helpers for writing files safely."""

from __future__ import annotations

import os
import re
import tempfile
from pathlib import Path

_SAFE_ARG = re.compile(r"^[A-Za-z0-9_./+@%:,=-]+$")


def write_atomic(path: Path, data: str | bytes, mode: int = 0o600):
    """Write via a temp file in the same folder, then rename over the target.
    Never leaves a half-written file, and never follows a symlink planted at the
    target path (the rename replaces the link itself)."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = data.encode("utf-8") if isinstance(data, str) else data
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as f:
            f.write(raw)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def private_dir(path: Path) -> Path:
    """Create a folder only the user can read (layouts can contain window titles)."""
    path = Path(path)
    path.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(path, 0o700)
    except OSError:
        pass
    return path


def desktop_value(s: str) -> str:
    """A string value for a .desktop file: no control characters (a newline would
    start a new key, e.g. a forged Exec=), backslashes escaped."""
    s = "".join(c if c.isprintable() else " " for c in s)
    return s.replace("\\", "\\\\")


def exec_arg(arg: str) -> str:
    """One argument of a .desktop Exec= line, quoted per the Desktop Entry spec."""
    if _SAFE_ARG.match(arg):
        return arg
    inner = "".join("\\" + c if c in '"`$\\' else c for c in arg)
    # the whole value is a string value too: its backslashes are escaped once more
    return '"' + inner.replace("\\", "\\\\") + '"'
