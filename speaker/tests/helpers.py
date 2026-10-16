"""Shared test setup: import path and file locations."""
import os
import sys

TESTS = os.path.dirname(os.path.abspath(__file__))
SPEAKER_DIR = os.path.dirname(TESTS)
REPO = os.path.dirname(SPEAKER_DIR)
FIXTURE = os.path.join(TESTS, "fixtures", "mini.md")
TALK = os.path.join(REPO, "TALK.md")
ENTRY = os.path.join(SPEAKER_DIR, "speaker")

if SPEAKER_DIR not in sys.path:
    sys.path.insert(0, SPEAKER_DIR)


class FakeClock:
    def __init__(self, t: float = 1000.0):
        self.t = t

    def __call__(self) -> float:
        return self.t


class FakeCopier:
    def __init__(self):
        self.copied = []

    def __call__(self, text):
        self.copied.append(text)
        return True, "fake"
