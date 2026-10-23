"""Draws demo.gif from real command output. Usage: render.py OUT.gif CMD1 FILE1 [CMD2 FILE2 ...]

Each command is "typed" then its captured output appears. Used instead of a screen recorder,
whose frames get dropped on a busy machine.
"""
import sys
from functools import reduce
from PIL import Image, ImageChops, ImageDraw, ImageFont

FONT = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 16)
BG, FG, DIM, GREEN = (24, 24, 24), (221, 221, 221), (120, 120, 120), (126, 211, 126)
CW = FONT.getbbox("M")[2]
LH = 22
PAD = 18

out_path, pairs = sys.argv[1], sys.argv[2:]
steps = [(pairs[i], open(pairs[i + 1]).read().rstrip("\n").split("\n")) for i in range(0, len(pairs), 2)]
cols = max(max(len(l) for l in lines) for _, lines in steps)
cols = max(cols, 70)
rows = max(len(lines) for _, lines in steps) + 2
W, H = cols * CW + 2 * PAD, rows * LH + 2 * PAD


def frame(prompt_cmd, lines, cursor=True):
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
    frames.append(img)
    durations.append(ms)

for cmd, lines in steps:
    for i in range(len(cmd) + 1):
        add(frame(cmd[:i], []), 70)
    add(frame(cmd, [], cursor=False), 350)
    add(frame(cmd, lines, cursor=False), 4500)

add(frame("", [], cursor=True), 800)
# palette from all output screens merged, so every text colour is represented
samples = [frame(cmd, lines, cursor=False) for cmd, lines in steps]
pal = reduce(ImageChops.lighter, samples).quantize(colors=64)
frames = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
frames[0].save(out_path, save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)
print(f"{out_path}: {len(frames)} frames, {W}x{H}")
