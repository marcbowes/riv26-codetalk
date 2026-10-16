"""End-to-end: run the real curses UI in a pseudo-terminal, press keys,
read the screen (via SPEAKER_SCREEN_DUMP) and the clipboard (via
SPEAKER_COPY_CMD), resize, and check the terminal is restored on exit."""
import fcntl
import os
import pty
import select
import shutil
import signal
import struct
import sys
import tempfile
import termios
import time
import unittest

import helpers
from prompter import parser
from prompter.model import RUN

# Arrow keys as a terminal in keypad mode sends them (ESC O C), and RAW-*
# as some terminals and presentation clickers send them (ESC [ C).
KEYS = {"RIGHT": b"\x1bOC", "LEFT": b"\x1bOD", "DOWN": b"\x1bOB", "UP": b"\x1bOA",
        "RAW-RIGHT": b"\x1b[C", "RAW-LEFT": b"\x1b[D", "PGDN": b"\x1b[6~", "PGUP": b"\x1b[5~",
        "ENTER": b"\r", "ESC": b"\x1b", "CTRL-C": b"\x03"}


class Session:
    def __init__(self, args, rows=40, cols=120, env=None):
        self.tmp = tempfile.mkdtemp(prefix="speaker-test-")
        self.dump = os.path.join(self.tmp, "screen.txt")
        self.clip = os.path.join(self.tmp, "clip.txt")
        full_env = dict(os.environ, TERM="xterm-256color", SPEAKER_SCREEN_DUMP=self.dump,
                        SPEAKER_COPY_CMD="cat > '%s'" % self.clip, LANG="en_US.UTF-8")
        full_env.pop("NO_COLOR", None)
        full_env.update(env or {})
        self.pid, self.fd = pty.fork()
        if self.pid == 0:  # child
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
                os.execve(sys.executable, [sys.executable, helpers.ENTRY] + list(args), full_env)
            finally:
                os._exit(127)
        self.out = b""
        self.status = None
        self.wait_for(lambda s: s.strip(), 5)

    def read(self, secs=0.2):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    return
                if not data:
                    return
                self.out += data

    def screen(self) -> str:
        try:
            with open(self.dump, encoding="utf-8") as fh:
                return fh.read()
        except OSError:
            return ""

    def wait_for(self, pred, secs=3.0):
        end = time.time() + secs
        while time.time() < end:
            self.read(0.05)
            s = self.screen()
            if pred(s):
                return s
        return self.screen()

    def press(self, *keys, settle=0.25):
        for k in keys:
            os.write(self.fd, KEYS.get(k, k.encode() if isinstance(k, str) else k))
            self.read(0.08)
        self.read(settle)
        return self.screen()

    def resize(self, rows, cols):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)
        return self.wait_for(lambda s: s and len(s.split("\n")[0]) == cols)

    def clipboard(self):
        end = time.time() + 2
        while time.time() < end:
            if os.path.exists(self.clip):
                with open(self.clip, encoding="utf-8") as fh:
                    return fh.read()
            time.sleep(0.05)
        return None

    def finish(self, secs=5.0):
        end = time.time() + secs
        while time.time() < end:
            self.read(0.05)
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.read(0.1)
                self.status = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") \
                    else status >> 8
                return self.status
        os.kill(self.pid, signal.SIGKILL)
        os.waitpid(self.pid, 0)
        raise AssertionError("speaker did not exit")

    def close(self):
        try:
            os.close(self.fd)
        except OSError:
            pass
        shutil.rmtree(self.tmp, ignore_errors=True)

    @property
    def restored(self) -> bool:
        # rmcup (leave the alternate screen) and the cursor shown again
        return b"\x1b[?1049l" in self.out and b"\x1b[?25h" in self.out


@unittest.skipUnless(hasattr(pty, "fork"), "needs a pty")
class Fixture(unittest.TestCase):
    def test_navigate_copy_resize_quit(self):
        s = Session([helpers.FIXTURE])
        try:
            scr = s.screen()
            self.assertIn("Opening", scr)
            self.assertIn("beat 1/1", scr)
            self.assertIn("Hello and welcome.", scr)
            self.assertIn("PRESENTER NOTES", scr)
            scr = s.press("n", " ", "RIGHT")
            self.assertIn("Step 1 · Make a table", scr)
            self.assertIn("beat 3/7", scr)
            self.assertIn("▌", scr)
            self.assertIn("NEXT ▸", scr)
            scr = s.press("c")
            self.assertEqual(s.clipboard(), "CREATE TABLE t (id int PRIMARY KEY);")
            self.assertIn("COPIED 1 line for psql · pane B", scr)
            scr = s.press("LEFT")
            self.assertIn("beat 2/7", scr)
            scr = s.press("RAW-RIGHT", "PGDN", "PGUP", "RAW-LEFT")
            self.assertIn("beat 2/7", scr)
            self.assertNotIn("Nothing to copy", scr)   # no stray 'C' from ESC [ C
            scr = s.press("s")
            self.assertIn("Clock running", scr)
            scr = s.press("f")
            self.assertIn("READER NOTE", scr)
            scr = s.press("g", "r", "a", "c", "e")
            self.assertIn("go to step", scr)
            scr = s.press("ENTER")
            self.assertIn("Steps 2 and 3 · Race and load", scr)
            scr = s.press("?")
            self.assertIn("speaker · keys", scr)
            s.press("x")
            scr = s.resize(20, 60)
            self.assertEqual(len(scr.split("\n")[0]), 60)
            self.assertIn("Race and load", scr)
            scr = s.resize(8, 30)
            self.assertEqual(len(scr.split("\n")[0]), 30)
            s.press("q")
            self.assertEqual(s.finish(), 0)
            self.assertTrue(s.restored)
            self.assertIn(b"stopped at Steps 2 and 3", s.out)
        finally:
            s.close()

    def test_ctrl_c_restores_terminal(self):
        s = Session([helpers.FIXTURE], rows=24, cols=80)
        try:
            s.press("n", "n")
            s.press("CTRL-C")
            self.assertEqual(s.finish(), 0)
            self.assertTrue(s.restored)
            self.assertNotIn(b"Traceback", s.out)
        finally:
            s.close()

    def test_no_color(self):
        for env in ({"NO_COLOR": "1"}, {"TERM": "vt100"}):
            s = Session([helpers.FIXTURE], rows=24, cols=80, env=env)
            try:
                scr = s.press("n", "n", "n")
                self.assertIn("▶ RUN", scr)
                s.press("q")
                self.assertEqual(s.finish(), 0, env)
                self.assertNotIn(b"Traceback", s.out)
                if env.get("NO_COLOR"):
                    self.assertNotIn(b"\x1b[38;5;", s.out)
            finally:
                s.close()


@unittest.skipUnless(hasattr(pty, "fork") and os.path.exists(helpers.TALK), "needs a pty and TALK.md")
class RealTalk(unittest.TestCase):
    def test_play_the_real_talk(self):
        talk = parser.load(helpers.TALK)
        s = Session([helpers.TALK], rows=45, cols=150)
        try:
            scr = s.screen()
            self.assertIn("Intro", scr)                     # starts at the intro, not the preamble
            self.assertIn("CLOCK", scr)                     # delta badge says the clock is stopped
            self.assertIn("0:00 / 45:00", scr)
            # go to step 6 and walk to its first command
            st = next(x for x in talk.steps if 6 in x.numbers)
            first_run = next(i for i, b in enumerate(st.beats) if b.kind == RUN)
            s.press("g", "6", "ENTER")
            scr = s.press(*(["n"] * first_run))
            self.assertIn(st.name[:30], scr)
            self.assertIn("beat %d/%d" % (first_run + 1, len(st.beats)), scr)
            s.press("c")
            self.assertEqual(s.clipboard(), st.beats[first_run].command)
            # the clock and delta
            scr = s.press("s", settle=1.3)
            self.assertNotIn("STOPPED", scr.split("\n")[1])
            s.press("q")
            self.assertEqual(s.finish(), 0)
            self.assertTrue(s.restored)
        finally:
            s.close()


if __name__ == "__main__":
    unittest.main()
