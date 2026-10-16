import unittest

import helpers  # noqa: F401  (sets sys.path)
from prompter import timing


class ParseTime(unittest.TestCase):
    def test_minutes_seconds(self):
        self.assertEqual(timing.parse_time("17:00"), 1020)
        self.assertEqual(timing.parse_time("0:00"), 0)
        self.assertEqual(timing.parse_time("10:30"), 630)
        self.assertEqual(timing.parse_time(" 5:00 "), 300)

    def test_hours(self):
        self.assertEqual(timing.parse_time("1:02:03"), 3723)

    def test_invalid(self):
        for bad in ("", "17", "17:6", "17:60", "abc", "1:60:00", None):
            self.assertIsNone(timing.parse_time(bad), bad)

    def test_ranges_with_any_dash(self):
        for text in ("17:00–20:00", "17:00-20:00", "17:00 — 20:00", "17:00 to 20:00", "17:00--20:00"):
            self.assertEqual(timing.parse_range(text), (1020, 1200), text)

    def test_range_inside_heading(self):
        self.assertEqual(timing.parse_range("Step 6 · An index · 17:00–20:00"), (1020, 1200))
        self.assertIsNone(timing.parse_range("Step 6 · An index · 17:00–20:00", whole=True))
        self.assertEqual(timing.parse_range("0:00–5:00", whole=True), (0, 300))

    def test_backwards_range_rejected(self):
        self.assertIsNone(timing.parse_range("20:00–17:00"))

    def test_format(self):
        self.assertEqual(timing.fmt(1020), "17:00")
        self.assertEqual(timing.fmt(65), "1:05")
        self.assertEqual(timing.fmt(3723), "1:02:03")
        self.assertEqual(timing.fmt(83, signed=True), "+1:23")
        self.assertEqual(timing.fmt(-5), "-0:05")
        self.assertEqual(timing.fmt(None), "--:--")
        self.assertEqual(timing.fmt_range(1020, 1200), "17:00–20:00")


class StepPace(unittest.TestCase):
    """The per-step window indicator."""

    def test_states(self):
        self.assertEqual(timing.pace(1000, 1020, 1200)[0], "ahead")
        self.assertEqual(timing.pace(1020, 1020, 1200)[0], "ok")
        self.assertEqual(timing.pace(1139, 1020, 1200)[0], "ok")
        self.assertEqual(timing.pace(1141, 1020, 1200)[0], "warn")   # last minute
        self.assertEqual(timing.pace(1200, 1020, 1200)[0], "over")
        self.assertEqual(timing.pace(1283, 1020, 1200), ("over", "OVER +1:23"))
        self.assertEqual(timing.pace(10, None, None)[0], "none")

    def test_short_step_warns_for_half(self):
        # a one-minute step is amber for its last 30 s, not all of it
        self.assertEqual(timing.warn_window(1980, 2040), 30)
        self.assertEqual(timing.pace(1990, 1980, 2040)[0], "ok")
        self.assertEqual(timing.pace(2015, 1980, 2040)[0], "warn")


class ScheduleDelta(unittest.TestCase):
    """The big ahead/behind badge: delta = clock - planned position."""

    def test_planned_at_interpolates_across_beats(self):
        # Step 6, 17:00-20:00, 12 beats: at beat 0 you should be at 17:00,
        # halfway through the beats at 18:30, past the last beat at 20:00.
        self.assertEqual(timing.planned_at(1020, 1200, 0, 12), 1020)
        self.assertEqual(timing.planned_at(1020, 1200, 6, 12), 1110)
        self.assertEqual(timing.planned_at(1020, 1200, 3, 12), 1065)
        self.assertEqual(timing.planned_at(1020, 1200, 12, 12), 1200)
        self.assertEqual(timing.planned_at(1020, 1200, 99, 12), 1200)   # clamped
        self.assertEqual(timing.planned_at(0, 300, 0, 0), 0)            # no beats
        self.assertIsNone(timing.planned_at(None, None, 0, 3))

    def test_before_clock_starts(self):
        self.assertEqual(timing.schedule(0, False, 1020), ("none", "--", "CLOCK STOPPED"))

    def test_off_script(self):
        self.assertEqual(timing.schedule(100, True, None)[:2], ("none", "--"))

    def test_on_time(self):
        self.assertEqual(timing.schedule(1110, True, 1110), ("ok", "±0", "ON TIME"))
        self.assertEqual(timing.schedule(1110 + 29, True, 1110)[0], "ok")
        self.assertEqual(timing.schedule(1110 - 29, True, 1110)[0], "ok")

    def test_behind_amber_then_red(self):
        self.assertEqual(timing.schedule(1110 + 45, True, 1110), ("warn", "+45s", "BEHIND"))
        self.assertEqual(timing.schedule(1110 + 89, True, 1110)[0], "warn")
        self.assertEqual(timing.schedule(1110 + 90, True, 1110), ("over", "+1m", "BEHIND"))
        self.assertEqual(timing.schedule(1110 + 120, True, 1110), ("over", "+2m", "BEHIND"))
        self.assertEqual(timing.schedule(1110 + 600, True, 1110), ("over", "+10m", "BEHIND"))

    def test_ahead_is_green(self):
        self.assertEqual(timing.schedule(1110 - 60, True, 1110), ("ahead", "-1m", "AHEAD"))
        self.assertEqual(timing.schedule(1110 - 40, True, 1110), ("ahead", "-40s", "AHEAD"))

    def test_mid_step(self):
        # 6 of 12 beats into 17:00-20:00 means you should be at 18:30.
        planned = timing.planned_at(1020, 1200, 6, 12)
        self.assertEqual(timing.schedule(18 * 60 + 30, True, planned)[0], "ok")
        self.assertEqual(timing.schedule(20 * 60 + 30, True, planned)[1], "+2m")
        self.assertEqual(timing.schedule(17 * 60 + 30, True, planned)[1], "-1m")

    def test_planned_window(self):
        self.assertEqual(timing.planned_window(0, 300, 0, 1), (0, 300))          # one-beat step
        self.assertEqual(timing.planned_window(1020, 1200, 6, 12), (1110, 1125))
        self.assertEqual(timing.planned_window(1020, 1200, 11, 12), (1185, 1200))
        self.assertIsNone(timing.planned_window(None, None, 0, 1))

    def test_on_time_anywhere_inside_the_beat(self):
        # The bug: a one-beat intro at 2:33 read "+3m BEHIND".
        intro = timing.planned_window(0, 300, 0, 1)
        self.assertEqual(timing.schedule(153, True, intro), ("ok", "±0", "ON TIME"))
        self.assertEqual(timing.schedule(1, True, intro)[0], "ok")
        self.assertEqual(timing.schedule(299, True, intro)[0], "ok")
        # Past the beat's end: behind by the overrun, not by the whole beat.
        self.assertEqual(timing.schedule(300 + 45, True, intro), ("warn", "+45s", "BEHIND"))
        self.assertEqual(timing.schedule(300 + 150, True, intro), ("over", "+2m", "BEHIND"))
        # Before the beat's start: ahead by the gap.
        mid = timing.planned_window(1020, 1200, 6, 12)                          # 18:30-18:45
        self.assertEqual(timing.schedule(1050, True, mid), ("ahead", "-1m", "AHEAD"))
        self.assertEqual(timing.schedule(1115, True, mid)[0], "ok")
        self.assertEqual(timing.schedule(1125 + 60, True, mid), ("warn", "+1m", "BEHIND"))

    def test_fmt_delta(self):
        self.assertEqual(timing.fmt_delta(10), "±0")
        self.assertEqual(timing.fmt_delta(-10), "±0")
        self.assertEqual(timing.fmt_delta(59), "+59s")
        self.assertEqual(timing.fmt_delta(60), "+1m")
        self.assertEqual(timing.fmt_delta(150), "+2m")   # whole minutes, rounded down
        self.assertEqual(timing.fmt_delta(-125), "-2m")


if __name__ == "__main__":
    unittest.main()
