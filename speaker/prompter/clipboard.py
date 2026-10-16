"""Copy text to the system clipboard.

Order: $SPEAKER_COPY_CMD (a shell command that reads stdin), pbcopy,
wl-copy, xclip, xsel, then the OSC 52 terminal escape as a last resort.
"""
from __future__ import annotations

import base64
import os
import shutil
import subprocess
from typing import Tuple


def copy(text: str) -> Tuple[bool, str]:
    """Returns (ok, how-or-error)."""
    data = text.encode("utf-8")
    env = dict(os.environ)
    # pbcopy decodes stdin with the locale; make sure that's UTF-8
    if "UTF-8" not in (env.get("LC_ALL", "") + env.get("LC_CTYPE", "") + env.get("LANG", "")).upper():
        env["LC_CTYPE"] = "UTF-8"
        env["LANG"] = env.get("LANG") or "en_US.UTF-8"
    custom = os.environ.get("SPEAKER_COPY_CMD")
    candidates = []
    if custom:
        candidates.append(("$SPEAKER_COPY_CMD", custom, True))
    for name, cmd in (("pbcopy", ["pbcopy"]), ("wl-copy", ["wl-copy"]),
                      ("xclip", ["xclip", "-selection", "clipboard"]),
                      ("xsel", ["xsel", "--clipboard", "--input"])):
        if shutil.which(cmd[0]):
            candidates.append((name, cmd, False))
    for name, cmd, shell in candidates:
        try:
            r = subprocess.run(cmd, input=data, shell=shell, env=env, timeout=3,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if r.returncode == 0:
                return True, name
        except (OSError, subprocess.SubprocessError):
            continue
    return _osc52(data)


def _osc52(data: bytes) -> Tuple[bool, str]:
    try:
        seq = b"\x1b]52;c;" + base64.b64encode(data) + b"\x07"
        if os.environ.get("TMUX"):
            seq = b"\x1bPtmux;\x1b" + seq + b"\x1b\\"
        with open("/dev/tty", "wb", buffering=0) as tty:
            tty.write(seq)
        return True, "terminal (OSC 52)"
    except OSError as exc:
        return False, "no clipboard tool found (%s)" % exc
