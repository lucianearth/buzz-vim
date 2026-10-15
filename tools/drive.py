#!/usr/bin/env python3
"""Drive the real buzz-vim binary in a pseudo-terminal and read the screen.

Usage: drive.py [--cols 120 --rows 36 --raw out.bin] BINARY -- STEP...

Steps (each one argument):
  keys:<text>       type text literally
  key:<name>        esc enter tab backspace up down left right ctrl-v ctrl-c ...
  paste:<text>      bracketed paste (what a terminal sends on drag-and-drop)
  click:<col>,<row> left click (1-based), SGR mouse encoding
  wheel:up|down@<col>,<row>
  wait:<text>       wait until text is on screen (10 s), fail otherwise
  gone:<text>       wait until text is not on screen
  sleep:<secs>
  shot              print the screen
  find:<text>       print "<col>,<row>" of the first match
  clickon:<text>    left click the first place <text> is on screen
  png:<path>        save the screen as a PNG, colours and all (needs Pillow and
                    DejaVu Sans Mono; set BUZZ_VIM_FONT_DIR to its folder)

Needs pyte (python -m pip install pyte).
"""
import os, pty, select, sys, time, fcntl, termios, struct, argparse
import pyte

KEYS = {
    "esc": b"\x1b", "enter": b"\r", "tab": b"\t", "backspace": b"\x7f",
    "up": b"\x1b[A", "down": b"\x1b[B", "right": b"\x1b[C", "left": b"\x1b[D",
    "home": b"\x1b[H", "end": b"\x1b[F", "pgup": b"\x1b[5~", "pgdn": b"\x1b[6~",
    "shift-tab": b"\x1b[Z", "space": b" ", "delete": b"\x1b[3~",
}


def ctrl(c):
    return bytes([ord(c.lower()) - 96])


class Term:
    def __init__(self, argv, cols, rows, raw_path):
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.raw = open(raw_path, "wb") if raw_path else None
        pid, fd = pty.fork()
        if pid == 0:
            os.environ["TERM"] = os.environ.get("DRIVE_TERM", "xterm-256color")
            os.execvp(argv[0], argv)
        self.pid, self.fd = pid, fd
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, cols * 8, rows * 16))

    def pump(self, secs=0.05):
        end = time.time() + secs
        while True:
            left = end - time.time()
            if left <= 0:
                return
            r, _, _ = select.select([self.fd], [], [], left)
            if not r:
                continue
            try:
                data = os.read(self.fd, 1 << 16)
            except OSError:
                return
            if not data:
                return
            if self.raw:
                self.raw.write(data)
            # Answer terminal queries the way a plain xterm would, so
            # capability probes don't stall.
            if b"\x1b[c" in data or b"\x1b[0c" in data:
                os.write(self.fd, b"\x1b[?62;22c")
            if b"\x1b[16t" in data:
                os.write(self.fd, b"\x1b[6;16;8t")
            if b"\x1b[14t" in data:
                os.write(self.fd, b"\x1b[4;%d;%dt" % (self.screen.lines * 16, self.screen.columns * 8))
            self.stream.feed(data)

    def send(self, b):
        os.write(self.fd, b)
        self.pump(0.15)

    def text(self):
        return "\n".join(line.rstrip() for line in self.screen.display)

    def wait(self, needle, present=True, timeout=10.0):
        end = time.time() + timeout
        while time.time() < end:
            self.pump(0.1)
            if (needle in self.text()) == present:
                return True
        return False

    def find(self, needle):
        for y, line in enumerate(self.screen.display):
            x = line.find(needle)
            if x >= 0:
                return x + 1, y + 1
        return None


NAMED = {
    "black": (0, 0, 0), "red": (205, 49, 49), "green": (13, 188, 121),
    "brown": (229, 229, 16), "blue": (36, 114, 200), "magenta": (188, 63, 188),
    "cyan": (17, 168, 205), "white": (229, 229, 229),
}


def save_png(screen, path):
    from PIL import Image, ImageDraw, ImageFont
    d = os.environ.get("BUZZ_VIM_FONT_DIR", "/usr/share/fonts/truetype/dejavu")
    size = 16
    reg = ImageFont.truetype(os.path.join(d, "DejaVuSansMono.ttf"), size)
    bold = ImageFont.truetype(os.path.join(d, "DejaVuSansMono-Bold.ttf"), size)
    cw = int(reg.getlength("M"))
    ch = int(size * 1.25)
    img = Image.new("RGB", (cw * screen.columns, ch * screen.lines), (0, 0, 0))
    dr = ImageDraw.Draw(img)

    def rgb(c, default):
        if c == "default":
            return default
        if c in NAMED:
            return NAMED[c]
        try:
            return tuple(int(c[i:i + 2], 16) for i in (0, 2, 4))
        except ValueError:
            return default

    for y in range(screen.lines):
        row = screen.buffer[y]
        for x in range(screen.columns):
            c = row[x]
            fg, bg = rgb(c.fg, (204, 204, 204)), rgb(c.bg, (0, 0, 0))
            if c.reverse:
                fg, bg = bg, fg
            dr.rectangle([x * cw, y * ch, (x + 1) * cw - 1, (y + 1) * ch - 1], fill=bg)
            if c.data.strip():
                if c.data in "▀▄█▌▐▎▏":
                    # Draw block elements as boxes so they tile.
                    box = {"▀": (0, 0, 1, .5), "▄": (0, .5, 1, 1), "█": (0, 0, 1, 1),
                           "▌": (0, 0, .5, 1), "▐": (.5, 0, 1, 1), "▎": (0, 0, .25, 1),
                           "▏": (0, 0, .125, 1)}[c.data]
                    dr.rectangle([x * cw + box[0] * cw, y * ch + box[1] * ch,
                                  x * cw + box[2] * cw - 1, y * ch + box[3] * ch - 1], fill=fg)
                else:
                    dr.text((x * cw, y * ch + 1), c.data, font=bold if c.bold else reg, fill=fg)
    img.save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cols", type=int, default=120)
    ap.add_argument("--rows", type=int, default=36)
    ap.add_argument("--raw")
    ap.add_argument("binary")
    ap.add_argument("steps", nargs="*")
    a = ap.parse_args()
    t = Term([a.binary], a.cols, a.rows, a.raw)
    t.pump(0.5)
    ok = True
    for step in a.steps:
        kind, _, arg = step.partition(":")
        if kind == "keys":
            for ch in arg:
                t.send(ch.encode())
        elif kind == "key":
            t.send(KEYS[arg] if arg in KEYS else ctrl(arg[5:]) if arg.startswith("ctrl-") else arg.encode())
        elif kind == "paste":
            t.send(b"\x1b[200~" + arg.encode() + b"\x1b[201~")
        elif kind == "click":
            x, y = arg.split(",")
            t.send(f"\x1b[<0;{x};{y}M".encode())
            t.send(f"\x1b[<0;{x};{y}m".encode())
        elif kind == "clickon":
            t.pump(0.3)
            pos = t.find(arg)
            if not pos:
                print(f"FAIL clickon:{arg}")
                ok = False
                break
            t.send(f"\x1b[<0;{pos[0]};{pos[1]}M".encode())
            t.send(f"\x1b[<0;{pos[0]};{pos[1]}m".encode())
        elif kind == "wheel":
            d, _, pos = arg.partition("@")
            x, y = pos.split(",")
            t.send(f"\x1b[<{64 if d == 'up' else 65};{x};{y}M".encode())
        elif kind == "wait":
            if not t.wait(arg):
                print(f"FAIL wait:{arg}")
                ok = False
                break
        elif kind == "gone":
            if not t.wait(arg, present=False):
                print(f"FAIL gone:{arg}")
                ok = False
                break
        elif kind == "sleep":
            t.pump(float(arg))
        elif kind == "shot":
            t.pump(0.3)
            print("-" * a.cols)
            print(t.text())
            print("-" * a.cols)
        elif kind == "png":
            t.pump(0.3)
            save_png(t.screen, arg)
        elif kind == "find":
            print("FOUND", t.find(arg))
        else:
            print(f"unknown step {step}")
            ok = False
    if not ok:
        print(t.text())
    try:
        os.kill(t.pid, 15)
    except OSError:
        pass
    t.pump(0.3)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
