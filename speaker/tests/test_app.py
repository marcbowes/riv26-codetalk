"""Key handling and navigation, without a terminal."""
import curses
import os
import shutil
import tempfile
import unittest

import helpers
from prompter import parser
from prompter.app import App, search
from prompter.model import PROMPT, RACE, RUN, SAY


def keys(app, *ks):
    for k in ks:
        assert app.handle_key(k) is not False or k == "q"


class Navigation(unittest.TestCase):
    def setUp(self):
        self.clock = helpers.FakeClock(0)
        self.copier = helpers.FakeCopier()
        self.app = App(parser.load(helpers.FIXTURE), helpers.FIXTURE, now=self.clock, copier=self.copier)

    def test_starts_at_first_talk_step(self):
        self.assertEqual(self.app.step.name, "Opening")
        self.assertEqual(self.app.beat, 0)

    def test_n_walks_beats_then_steps_in_flow(self):
        a = self.app
        keys(a, "n")                       # Opening has one beat
        self.assertEqual((a.step.name, a.beat), ("Step 1 · Make a table", 0))
        keys(a, " ", curses.KEY_RIGHT)
        self.assertEqual(a.beat, 2)
        keys(a, "p", curses.KEY_LEFT, "p")  # back past the start of the step
        self.assertEqual((a.step.name, a.beat), ("Opening", 0))
        keys(a, "N", "N")
        self.assertEqual(a.step.name, "Steps 2 and 3 · Race and load")
        keys(a, "P")
        self.assertEqual((a.step.name, a.beat), ("Step 1 · Make a table", 0))
        keys(a, "p")
        self.assertEqual(a.step.name, "Opening")

    def test_end_of_talk_does_not_wander_into_cleanup(self):
        a = self.app
        for _ in range(40):
            keys(a, "n")
        self.assertEqual(a.step.name, "Step 4 · Wrap the build")
        self.assertIn("End of the talk", a.current_flash()[0])

    def test_goto_by_number_title_and_time(self):
        a = self.app
        keys(a, "g", "3", "\n")
        self.assertEqual(a.step.name, "Steps 2 and 3 · Race and load")
        keys(a, "g", *"wrap", "\n")
        self.assertEqual(a.step.name, "Step 4 · Wrap the build")
        keys(a, "g", *"3:00", "\n")
        self.assertEqual(a.step.name, "Step 1 · Make a table")
        keys(a, "g", *"clean", "\n")
        self.assertEqual(a.step.name, "Cleaning up")
        self.assertFalse(a.step.talk)
        # from outside the flow, p walks the whole document
        keys(a, "p")
        self.assertEqual(a.step.name, "Step 4 · Wrap the build")

    def test_goto_escape_and_selection(self):
        a = self.app
        keys(a, "g", "x", curses.KEY_BACKSPACE, curses.KEY_DOWN, curses.KEY_DOWN, curses.KEY_UP)
        self.assertEqual(a.sel, 1)
        keys(a, "\x1b")
        self.assertEqual((a.mode, a.step.name), ("normal", "Opening"))

    def test_search_ranks_numbers_first(self):
        steps = self.app.steps
        self.assertEqual(steps[search(steps, "1")[0]].name, "Step 1 · Make a table")
        self.assertEqual(steps[search(steps, "race")[0]].name, "Steps 2 and 3 · Race and load")
        self.assertEqual(len(search(steps, "")), len(steps))
        self.assertEqual(search(steps, "zzzzqqq"), [])

    def test_copy_command_prompt_and_race_row(self):
        a = self.app
        keys(a, "g", "1", "\n", "n", "n")
        self.assertEqual(a.current.kind, RUN)
        keys(a, "c")
        self.assertEqual(self.copier.copied[-1], "CREATE TABLE t (id int PRIMARY KEY);")
        self.assertIn("COPIED", a.current_flash()[0])
        keys(a, "n", "n", "n")
        self.assertEqual(a.current.kind, PROMPT)
        keys(a, "c")
        self.assertIn("<cluster-endpoint>.", self.copier.copied[-1])
        self.assertIn("fill in <cluster-endpoint>", a.current_flash()[0])
        keys(a, "g", "2", "\n", "n", "n", "n", "n")
        self.assertEqual(a.current.kind, RACE)
        keys(a, "c")
        self.assertEqual(self.copier.copied[-1], "COMMIT;")

    def test_copy_pieces_cycle(self):
        a = self.app
        keys(a, "g", "2", "\n")
        self.assertEqual(a.current.kind, RUN)
        keys(a, "c")
        self.assertIn("\\gset", a.current_flash()[0])        # warned about pasting \gset pairs
        keys(a, "C")
        self.assertEqual(self.copier.copied[-1], "SELECT 1 AS x \\gset")
        keys(a, "C")
        self.assertEqual(self.copier.copied[-1], "SELECT 2 AS y \\gset")
        keys(a, "C")
        self.assertEqual(self.copier.copied[-1], "SELECT 1 AS x \\gset")   # wraps around

    def test_copy_nothing_on_say(self):
        a = self.app
        keys(a, "c")
        self.assertEqual(self.copier.copied, [])
        self.assertEqual(a.current_flash()[1], "warn")

    def test_clock_and_delta(self):
        a = self.app
        self.assertEqual(a.status()["delta"][1], "--")
        keys(a, "s")
        self.clock.t += 60
        self.assertTrue(a.running)
        self.assertAlmostEqual(a.elapsed(), 60)
        keys(a, "s")                       # pause
        self.clock.t += 50
        self.assertAlmostEqual(a.elapsed(), 60)
        self.assertEqual(a.status()["delta"][:2], ("warn", "+1m"))   # 60 s into a 0:00 plan
        keys(a, "s")
        self.clock.t += 40
        self.assertEqual(a.status()["delta"][:2], ("over", "+2m"))   # 100 s: red
        keys(a, "g", "4", "\n")            # Step 4 planned at 8:00, clock 1:40
        self.assertEqual(a.status()["delta"][:2], ("ahead", "-6m"))
        self.assertEqual(a.status()["delta"][2], "AHEAD")
        keys(a, "t")
        self.assertEqual(a.elapsed(), 0)
        self.assertEqual(a.status()["delta"][1], "--")

    def test_delta_moves_with_beats(self):
        a = self.app
        keys(a, "g", "1", "\n")            # Step 1: 2:00-4:00, 7 beats
        a.set_clock(140)
        a.toggle_clock()
        self.assertEqual(a.planned(), 120)
        self.assertEqual(a.status()["delta"][0], "ok")      # 20 s behind: on time
        keys(a, "n", "n", "n")
        self.assertAlmostEqual(a.planned(), 120 + 120 * 3 / 7)   # 3 of 7 beats passed
        self.assertEqual(a.status()["delta"][0], "ahead")  # 140 vs 171 s

    def test_toggles_help_and_quit(self):
        a = self.app
        keys(a, "f")
        self.assertTrue(a.show_reader)
        keys(a, "?")
        self.assertEqual(a.mode, "help")
        self.assertTrue(a.handle_key("q"))   # q closes help, doesn't quit
        self.assertEqual(a.mode, "normal")
        self.assertFalse(a.handle_key("q"))

    def test_scroll_keys(self):
        a = self.app
        keys(a, "j", "j", curses.KEY_DOWN)
        self.assertEqual(a.scroll_delta, 3)
        keys(a, "k")
        self.assertEqual(a.scroll_delta, 2)
        keys(a, "n")
        self.assertEqual(a.scroll_delta, 0)


class Reload(unittest.TestCase):
    def test_reload_keeps_position(self):
        tmp = tempfile.mkdtemp()
        try:
            path = os.path.join(tmp, "TALK.md")
            shutil.copy(helpers.FIXTURE, path)
            clock = helpers.FakeClock(0)
            a = App(parser.load(path), path, now=clock, copier=helpers.FakeCopier())
            keys(a, "g", "2", "\n", "n", "n", "n")
            self.assertEqual(a.beat, 3)
            with open(path, encoding="utf-8") as fh:
                text = fh.read()
            text = text.replace("Some more about the console, same beat.",
                                "Some more.\n\n**A new bold beat.** Inserted before.")
            text = text.replace("Look at the table:", "Look closely at the table:")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
            os.utime(path, (1, 1))
            clock.t += 5
            a.tick()
            self.assertTrue(a.changed_on_disk)
            keys(a, "r")
            self.assertFalse(a.changed_on_disk)
            self.assertEqual((a.step.name, a.beat), ("Steps 2 and 3 · Race and load", 3))
            st1 = next(s for s in a.steps if s.label == "Step 1")
            self.assertEqual(len(st1.beats), 8)
            self.assertIn("Reloaded", a.current_flash()[0])
        finally:
            shutil.rmtree(tmp)



class Escapes(unittest.TestCase):
    def _decode(self, chars):
        from prompter.app import decode_escape
        it = iter(chars)
        return decode_escape(lambda: next(it, None))

    def test_raw_sequences(self):
        self.assertEqual(self._decode("[C"), curses.KEY_RIGHT)
        self.assertEqual(self._decode("OD"), curses.KEY_LEFT)
        self.assertEqual(self._decode("[6~"), curses.KEY_NPAGE)
        self.assertEqual(self._decode("[5~"), curses.KEY_PPAGE)
        self.assertEqual(self._decode("[1;5C"), curses.KEY_RIGHT)
        self.assertEqual(self._decode(""), "\x1b")          # a lone Esc
        self.assertEqual(self._decode("[Z"), "\x1b")        # unknown: swallowed


if __name__ == "__main__":
    unittest.main()
