"""Draws the README demo animation from real command output.

Usage:  render.py OUT.gif CMD1 FILE1 [CMD2 FILE2 ...]
          OUT.gif  the animation to write
          CMD      the command line to show being typed, e.g. "safesweep scan"
          FILE     a text file holding that command's captured output

For each command the animation types it out letter by letter, pauses, then shows its output.
The output is exactly what the program printed; only the typing is simulated.

This is used instead of a screen recorder because recorders drop frames when the machine is
busy, which made the animation play far too fast. It needs Pillow (`pip install pillow`) and the
DejaVu Sans Mono font. Called by demo/make.sh.
"""
import sys
from functools import reduce
from PIL import Image, ImageChops, ImageDraw, ImageFont

# --- Appearance: a dark terminal, light text, green prompt; measurements in pixels. ---
FONT = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 16)
BG, FG, DIM, GREEN = (24, 24, 24), (221, 221, 221), (120, 120, 120), (126, 211, 126)
CW = FONT.getbbox("M")[2]  # width of one character cell (the font is monospaced)
LH = 22  # height of one text line
PAD = 18  # margin around the text

out_path, pairs = sys.argv[1], sys.argv[2:]
# One (command, output lines) pair per command given on the command line.
steps = [(pairs[i], open(pairs[i + 1]).read().rstrip("\n").split("\n")) for i in range(0, len(pairs), 2)]
# Size the picture to the widest and tallest output, so every frame has the same size.
cols = max(max(len(l) for l in lines) for _, lines in steps)
cols = max(cols, 70)
rows = max(len(lines) for _, lines in steps) + 2
W, H = cols * CW + 2 * PAD, rows * LH + 2 * PAD


def frame(prompt_cmd, lines, cursor=True):
    """Draws one screen: the prompt with `prompt_cmd` typed so far, a block cursor if `cursor`,
    and the command's output `lines` below. Table headings are dimmed. Returns a Pillow image."""
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    d.text((PAD, PAD), "$ ", font=FONT, fill=GREEN)
    d.text((PAD + 2 * CW, PAD), prompt_cmd, font=FONT, fill=FG)
    if cursor:
        x = PAD + (2 + len(prompt_cmd)) * CW
        d.rectangle([x, PAD + 2, x + CW - 2, PAD + LH - 3], fill=FG)
    for i, line in enumerate(lines):
        color = DIM if line.startswith(("CATEGORY", "PROGRAM", "    PID")) else FG
        d.text((PAD, PAD + (i + 1) * LH), line, font=FONT, fill=color)
    return img


frames, durations = [], []


def add(img, ms):
    """Appends a frame that stays on screen for `ms` milliseconds."""
    frames.append(img)
    durations.append(ms)

# The timeline: type each command (70 ms per letter), pause 350 ms as if pressing Enter, then
# hold the output for 4.5 s so it can be read.
for cmd, lines in steps:
    for i in range(len(cmd) + 1):
        add(frame(cmd[:i], []), 70)
    add(frame(cmd, [], cursor=False), 350)
    add(frame(cmd, lines, cursor=False), 4500)

add(frame("", [], cursor=True), 800)  # a short pause on an empty prompt before the loop restarts
# palette from all output screens merged, so every text colour is represented
samples = [frame(cmd, lines, cursor=False) for cmd, lines in steps]
pal = reduce(ImageChops.lighter, samples).quantize(colors=64)
# GIF allows 256 colours; one shared palette keeps the text crisp and the file small.
frames = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
frames[0].save(out_path, save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)
print(f"{out_path}: {len(frames)} frames, {W}x{H}")
