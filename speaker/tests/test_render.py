"""Render smoke tests: the pure layout and screen composition, plus the
curses blit against a fake screen."""
import curses
import os
import unittest

import helpers
from prompter import inline, parser
from prompter.app import App, Theme, _P, blit
from prompter.layout import layout_step, seg_width
from prompter.model import KEY, RACE, RUN
from prompter.screen import big, compose

SIZES = [(3, 10), (4, 20), (6, 24), (10, 30), (12, 40), (16, 72), (20, 50), (24, 80),
         (40, 120), (45, 160), (50, 220), (60, 60)]


def talks():
    out = [parser.load(helpers.FIXTURE)]
    if os.path.exists(helpers.TALK):
        out.append(parser.load(helpers.TALK))
    return out


def make_app(talk, clock=None):
    return App(talk, talk.path, now=clock or helpers.FakeClock(), copier=helpers.FakeCopier())


class Layout(unittest.TestCase):
    def test_lines_fit_every_width(self):
        for talk in talks():
            for st in talk.steps:
                n = len(st.beats)
                for width in (12, 20, 30, 45, 60, 80, 100, 160):
                    for cur in sorted({0, n // 2, n - 1}):
                        for reader in (False, True):
                            lines, ranges = layout_step(st, cur, width, reader)
                            for ln in lines:
                                self.assertLessEqual(seg_width(ln.segs), width,
                                                     "%s w=%d %r" % (st.name, width, ln.text))
                            self.assertIn(cur, ranges, st.name)

    def test_current_beat_has_gutter_and_past_is_dimmed(self):
        talk = parser.load(helpers.FIXTURE)
        st = next(s for s in talk.steps if s.label == "Step 1")
        lines, ranges = layout_step(st, 2, 80)
        a, b = ranges[2]
        self.assertTrue(all(ln.text.startswith("▌") for ln in lines[a:b + 1]))
        before = [ln for ln in lines if ln.beat is not None and ln.beat < 2]
        self.assertTrue(before)
        for ln in before:
            for _, style in ln.segs[1:]:
                self.assertEqual(style[1], "past")

    def test_reader_notes_hidden_unless_toggled(self):
        talk = parser.load(helpers.FIXTURE)
        st = next(s for s in talk.steps if s.label == "Step 1")
        hidden = "\n".join(ln.text for ln in layout_step(st, 0, 80, False)[0])
        shown = "\n".join(ln.text for ln in layout_step(st, 0, 80, True)[0])
        self.assertNotIn("aws thing create --name x", hidden)
        self.assertIn("aws thing create --name x", shown)
        self.assertIn("READER NOTE", shown)

    def test_labels(self):
        talk = parser.load(helpers.FIXTURE)
        st = next(s for s in talk.steps if s.label == "Step 1")
        text = "\n".join(ln.text for ln in layout_step(st, 3, 80)[0])
        for label in ("▶ RUN", "psql · pane B", "EXPECT", "★ KEY POINT", "✦ PROMPT", "AI agent"):
            self.assertIn(label, text)
        self.assertNotIn("**", text)
        self.assertNotIn("](", text)

    def test_race_table_grid_and_now_line(self):
        talk = parser.load(helpers.FIXTURE)
        st = next(s for s in talk.steps if s.numbers == [2, 3])
        lines, ranges = layout_step(st, 4, 80)
        text = [ln.text for ln in lines]
        self.assertTrue(any("Pane A" in t and "Pane B" in t for t in text))
        cur = [t for t in text if t.startswith("▌")]
        self.assertTrue(any("COMMIT; → ERROR" in t for t in cur), cur)
        self.assertTrue(any("NOW" in t for t in cur))
        # very narrow terminals stack the cells instead
        narrow = [ln.text for ln in layout_step(st, 4, 16)[0]]
        self.assertTrue(any("Pane B:" in t for t in narrow), narrow)

    def test_big_digits(self):
        rows = big("+12")
        self.assertEqual(len(rows), 3)
        self.assertTrue(all(set(r) <= set("█▀▄ ") for r in rows))
        self.assertEqual(big("--")[1], "▀▀▀ ▀▀▀")


class Compose(unittest.TestCase):
    def _check(self, app, h, w):
        cv = compose(app, h, w)
        rows = cv.text()
        self.assertEqual(len(rows), h)
        for r in rows:
            self.assertEqual(inline.width(r), w, repr(r))
        return rows

    def test_every_size_every_step(self):
        for talk in talks():
            app = make_app(talk)
            for i, st in enumerate(talk.steps):
                for beat in sorted({0, len(st.beats) - 1}):
                    app.goto(i, beat)
                    for h, w in SIZES:
                        self._check(app, h, w)

    def test_overlays_at_every_size(self):
        app = make_app(talks()[-1])
        for mode in ("help", "goto"):
            app.mode = mode
            app.query = "st"
            for h, w in SIZES:
                self._check(app, h, w)

    def test_header_shows_step_budget_clock_and_delta(self):
        clock = helpers.FakeClock(0)
        app = make_app(parser.load(helpers.FIXTURE), clock)
        app.goto(next(i for i, s in enumerate(app.steps) if s.label == "Step 1"), 0)
        rows = self._check(app, 30, 120)
        head = "\n".join(rows[:3])
        self.assertIn("Build ▸ Step 1 · Make a table", head)
        self.assertIn("beat 1/7", head)
        self.assertIn("2:00–4:00", head)
        self.assertIn("0:00 / 10:00", head)
        self.assertIn("STOPPED", head)       # delta is "--" before s
        app.toggle_clock()
        clock.t += 120 + 150                 # 4:30; beat 1 of step 1 is planned for 2:00-2:17
        head = "\n".join(self._check(app, 30, 120)[:3])
        self.assertIn("BEHIND", head)
        self.assertIn("MIN", head)
        small = self._check(app, 12, 60)[0]
        self.assertIn("+2m BEHIND", small)  # 2 m 13 s past the beat, rounded down

    def test_notes_inline_when_narrow_sidebar_when_wide(self):
        app = make_app(parser.load(helpers.FIXTURE))
        wide = "\n".join(self._check(app, 30, 140))
        narrow = "\n".join(self._check(app, 30, 80))
        for text in (wide, narrow):
            self.assertIn("PRESENTER NOTES", text)
            self.assertIn("Have the terminal open.", text)
        self.assertIn("RUN OF SHOW", wide)

    def test_next_line(self):
        app = make_app(parser.load(helpers.FIXTURE))
        rows = self._check(app, 24, 100)
        self.assertIn("NEXT ▸", rows[-2])
        self.assertIn("Step 1 · Make a table", rows[-2])


class FakeScreen:
    def __init__(self, h, w):
        self.h, self.w = h, w
        self.cells = [[" "] * w for _ in range(h)]
        self.attrs = {}
        self.refreshed = 0

    def erase(self):
        self.cells = [[" "] * self.w for _ in range(self.h)]

    def getmaxyx(self):
        return self.h, self.w

    def addstr(self, y, x, text, attr=0):
        if y >= self.h or x >= self.w:
            raise curses.error("out of range")
        for i, ch in enumerate(text):
            if x + i >= self.w:
                raise curses.error("wrap")
            self.cells[y][x + i] = ch
            self.attrs[(y, x + i)] = attr

    def refresh(self):
        self.refreshed += 1

    def text(self):
        return ["".join(r) for r in self.cells]


class Blit(unittest.TestCase):
    def test_blit_matches_canvas(self):
        app = make_app(talks()[-1])
        for h, w in ((24, 80), (40, 130)):
            cv = compose(app, h, w)
            scr = FakeScreen(h, w)
            blit(scr, cv, Theme(0))
            self.assertEqual(scr.refreshed, 1)
            want = cv.text()
            got = scr.text()
            for y in range(h - 1):
                self.assertEqual(got[y], want[y].replace("", ""))
            self.assertEqual(got[-1][:-1], want[-1][:-1])   # bottom-right cell is skipped

    def test_mono_theme_attrs(self):
        th = Theme(0)
        for role in _P:
            for state in ("", "past", "cur", "fut"):
                for flags in ("", "b", "c", "bci"):
                    self.assertIsInstance(th.attr((role, state, flags)), int)
        self.assertTrue(th.attr(("key", "cur", "")) & curses.A_REVERSE)
        self.assertTrue(th.attr(("delta.over", "", "b")) & curses.A_REVERSE)
        self.assertTrue(th.attr(("say", "past", "")) & curses.A_DIM)
        self.assertTrue(th.attr(("say", "cur", "")) & curses.A_BOLD)
        self.assertEqual(th.attr(("no.such.role", "cur", "")), th.attr(("no.such.role", "cur", "")))


if __name__ == "__main__":
    unittest.main()
