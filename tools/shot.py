#!/usr/bin/env python3
"""Turn a cell dump from `BUZZ_VIM_SHOT=dir cargo test shot_` into a PNG.

Usage: shot.py in.cells out.png   (needs Pillow; BUZZ_VIM_FONT_DIR as in drive.py)
"""
import os, sys

from drive import save_png


class Cell:
    def __init__(self, data, fg, bg, bold):
        self.data, self.fg, self.bg, self.bold, self.reverse = data, fg, bg, bold, False


def xterm(i):
    if i == "-":
        return "default"
    if len(i) == 6:
        return i
    i = int(i)
    if i >= 232:
        v = 8 + (i - 232) * 10
        return "%02x%02x%02x" % (v, v, v)
    if i >= 16:
        i -= 16
        lv = [0, 95, 135, 175, 215, 255]
        return "%02x%02x%02x" % (lv[i // 36], lv[i // 6 % 6], lv[i % 6])
    return "default"


class Screen:
    def __init__(self, path):
        lines = open(path, encoding="utf-8").read().split("\n")
        self.columns, self.lines = map(int, lines[0].split())
        cells = [l.split("\t") for l in lines[1:] if l]
        self.buffer = [
            [Cell(c[0], xterm(c[1]), xterm(c[2]), c[3] == "1")
             for c in cells[y * self.columns:(y + 1) * self.columns]]
            for y in range(self.lines)
        ]


if __name__ == "__main__":
    save_png(Screen(sys.argv[1]), sys.argv[2])
