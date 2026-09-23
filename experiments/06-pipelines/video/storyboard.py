"""Experiment 6 overview video: a pipeline is built step by step, and every
shot shows what that real pipeline produced on the LiDAR tile.

    python3 experiments/06-pipelines/video/storyboard.py            # run shots + frames
    ffmpeg -framerate 30 -i out/video/frames/frame_%05d.png -c:v libx264 -preset slow \
      -crf 30 -pix_fmt yuv420p experiments/06-pipelines/video/overview.mp4

Stage 1 runs `target/release/pipeline` once per shot and keeps its output and
PNGs in out/video/. Stage 2 composes 1280x720 frames with Pillow: the chain on
the left (new steps typed out), the result on the right, a caption below.
"""

import json
import re
import subprocess
import textwrap
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "out" / "video"
FRAMES = OUT / "frames"
BIN = ROOT / "target" / "release" / "pipeline"
TILE = "data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz"
FPS = 30
W, H = 1280, 720

MONO = ImageFont.truetype("/System/Library/Fonts/SFNSMono.ttf", 15)
MONO_SMALL = ImageFont.truetype("/System/Library/Fonts/SFNSMono.ttf", 13)
SANS = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 20)
SANS_SMALL = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 15)
SANS_BOLD = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 26)

BG, PANEL, INK, MUTED = "#ffffff", "#1e2127", "#1b1f24", "#5f6670"
CODE, CODE_DIM = "#e6e6e6", "#7c828c"
KIND = {
    "Source": "#4c78a8",
    "Transform": "#59a14f",
    "Command": "#e15759",
    "Query": "#b07aa1",
    "Sink": "#f28e2b",
}
STEP_KIND = {
    "read": "Source",
    **{k: "Transform" for k in ["filter", "calc", "sql", "bin", "hillshade", "materialize"]},
    **{k: "Command" for k in ["chart", "set", "undo"]},
    **{k: "Query" for k in ["count", "head", "get", "domain", "history", "schema", "explain", "query"]},
    **{k: "Sink" for k in ["render", "png", "write", "save"]},
}

READ = f"read {TILE} --statistics"
BUILDINGS = 'filter --vega "datum.classification == 6"'
CELLS = ('sql "SELECT floor(x/5)*5 AS cx, floor(y/5)*5 AS cy, max(z) AS h '
         'FROM input GROUP BY floor(x/5)*5, floor(y/5)*5"')
MAP = ["chart symbol --x cx --y cy --size 5", "set width 420", "set height 420"]
STYLE = ['set title "Buildings, 5 m cells"', "set fill #c44e52"]
TALL = 'filter --vega "datum.cx >= 657500 && datum.cy >= 6867500"'
HEIGHTS = [
    READ,
    BUILDINGS,
    'calc height --vega "floor((datum.z - 42) / 4) * 4"',
    'sql "SELECT height, count(*) AS points FROM input WHERE height BETWEEN 0 AND 32 GROUP BY height ORDER BY height"',
    "calc label --vega \"toString(datum.height) + ' m'\"",
    "chart bar --x label --y points",
    'set title "Building points per 4 m of height"',
]
SHADE = [
    READ,
    'filter --vega "datum.x < 657400 && datum.y > 6867600"',
    'sql "SELECT floor(x) AS cx, floor(y) AS cy, max(z) AS h FROM input WHERE classification = 2 OR classification = 6 GROUP BY floor(x), floor(y)"',
    "hillshade --x cx --y cy --z h --cell 1",
    'sql "SELECT cx, cy, round(hillshade, 2) AS shade, ST_AsText(ST_Point(cx, cy)) AS wkt FROM input"',
    "head 2",
]


def shots():
    """(title, explanation, semantics, steps, number of new steps, image step)."""
    s = []
    s.append(("A pipeline starts lazily",
              "`read` only plans the scan. `count` is a query: the first step that executes.",
              "sql", [READ, "count"], 2, None))
    s.append(("A Vega expression is a filter step",
              "Compiled to a DataFusion expression with Vega's truthiness.",
              "sql", [READ, BUILDINGS, "count"], 1, None))
    s.append(("SQL sees the chain so far as `input`",
              "Commands (red) describe the chart; `render` is the sink that materialises.",
              "sql", [READ, BUILDINGS, CELLS, *MAP, "render out/video/s3.png"], 5, "s3"))
    s.append(("Commands only change the chart",
              "The data plan is untouched; the command log grows.",
              "sql", [READ, BUILDINGS, CELLS, *MAP, *STYLE, "render out/video/s4.png"], 3, "s4"))
    s.append(("A late Vega filter, after SQL and after the commands",
              "DataFusion rewrites it through the aggregation and hands it to the LAZ scan.",
              "sql", [READ, BUILDINGS, CELLS, TALL, *MAP, *STYLE, "render out/video/s5.png"], [3], "s5"))
    s.append(("Queries read and change nothing",
              "`domain x` asks the data what the chart will use, like Vega's domain('x').",
              "sql", [READ, BUILDINGS, CELLS, TALL, *MAP, *STYLE, "domain x", "get title", "render out/video/s6.png"], 3, "s6"))
    s.append(("A command changes the chart …",
              "`set fill` is appended to the command log.",
              "sql", [READ, BUILDINGS, CELLS, TALL, *MAP, *STYLE, "set fill #4c78a8", "render out/video/s7.png"], 2, "s7"))
    s.append(("… and undo refolds the log without it",
              "The chart state is a fold of the command log.",
              "sql", [READ, BUILDINGS, CELLS, TALL, *MAP, *STYLE, "set fill #4c78a8", "undo", "history", "render out/video/s8.png"], 3, "s8"))
    s.append(("SQL semantics: toString(0) is '0.0'",
              "A histogram of building heights, labels built by a Vega expression.",
              "sql", [*HEIGHTS, "render out/video/s9.png"], 7, "s9"))
    s.append(("Vega semantics: toString(0) is '0'",
              "Same pipeline, JavaScript semantics: 99.5 % of edge-case rows agree with real Vega.",
              "vega", [*HEIGHTS, "render out/video/s10.png"], 0, "s10"))
    s.append(("Packages add functions and steps",
              "`hillshade` is a whole-dataset step. SedonaDB's ST_Point / ST_AsText are normal SQL.",
              "vega", [*SHADE, "png out/video/s11.png --x cx --y cy --value shade"], 6, "s11"))
    return s


def run(semantics, steps):
    cmd = [str(BIN), "--semantics", semantics, "--trace", " ! ".join(steps)]
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode:
        raise SystemExit(f"pipeline failed:\n{' ! '.join(steps)}\n{r.stderr}")
    trace = [l for l in r.stdout.splitlines() if re.match(r"^\s*[\d.]+ ms  \w+  ", l)]
    outputs = [b.strip() for b in r.stdout.split("\n\n") if b.strip() and not re.match(r"^\s*[\d.]+ ms", b.strip())]
    return outputs, trace


def stage1():
    OUT.mkdir(parents=True, exist_ok=True)
    results = []
    for i, (title, note, sem, steps, new, image) in enumerate(shots()):
        outputs, trace = run(sem, steps)
        results.append(dict(title=title, note=note, semantics=sem, steps=steps, new=new,
                            image=str(OUT / f"{image}.png") if image else None,
                            outputs=outputs, trace=trace))
        print(f"shot {i + 1}: {title} ({len(outputs)} outputs)")
    # The last shot: save, then replay in a fresh process and compare bytes.
    steps = [READ, BUILDINGS, CELLS, TALL, *MAP, *STYLE, "render out/video/s12.png", "save out/video/pipeline.json"]
    outputs, trace = run("sql", steps)
    r = subprocess.run([str(BIN), "--replay", "out/video/pipeline.json", "render out/video/s12_replayed.png"],
                       cwd=ROOT, capture_output=True, text=True)
    same = (OUT / "s12.png").read_bytes() == (OUT / "s12_replayed.png").read_bytes()
    results.append(dict(
        title="Save the pipeline, replay it in a fresh process",
        note="Data steps + command log as JSON, like GDAL's .gdalg.json.",
        semantics="sql",
        steps=steps + ["$ pipeline --replay pipeline.json", "render replayed.png"],
        new=3, image=str(OUT / "s12_replayed.png"),
        outputs=[r.stdout.strip().splitlines()[0], "byte-identical PNG ✓" if same else "PNG differs ✗"],
        trace=trace))
    print("shot 12: replay", "identical" if same else "DIFFERS")
    (OUT / "shots.json").write_text(json.dumps(results, indent=1))


# ---------------------------------------------------------------------------

def wrap_step(step, width=53):
    lines = textwrap.wrap(step, width=width, subsequent_indent="    ", break_long_words=True)
    return lines or [""]


def new_indices(shot):
    n, new = len(shot["steps"]), shot["new"]
    return list(range(n - new, n)) if isinstance(new, int) else list(new)


def draw_panel(d, shot, typed_chars, cursor):
    d.rectangle([0, 0, 560, H], fill=PANEL)
    d.text((24, 22), "avenger pipeline", font=MONO, fill=CODE_DIM)
    d.text((24, 42), f"semantics: {shot['semantics']}", font=MONO_SMALL, fill=CODE_DIM)
    y = 76
    new = new_indices(shot)
    budget = typed_chars
    for i, step in enumerate(shot["steps"]):
        partial = False
        if i in new:
            if budget <= 0:
                continue
            shown, partial = step[:budget], budget < len(step)
            budget -= len(step)
        else:
            shown = step
        name = step.split()[0]
        kind = STEP_KIND.get(name, "Sink" if step.startswith("$") else "Transform")
        prefix = "  " if step.startswith("$") else "! "
        lines = wrap_step(shown)
        d.rectangle([24, y + 3, 28, y + 3 + 19 * len(lines) - 4], fill=KIND[kind])
        for j, line in enumerate(lines):
            d.text((38, y), (prefix if j == 0 else "  ") + line, font=MONO, fill=CODE if i in new else CODE_DIM)
            y += 19
        if partial and cursor:
            cx = 38 + d.textlength((prefix if len(lines) == 1 else "  ") + lines[-1], font=MONO) + 2
            d.rectangle([cx, y - 17, cx + 8, y - 2], fill=CODE)
        y += 6
    # legend
    lx, ly = 24, H - 34
    for k, c in KIND.items():
        d.rectangle([lx, ly + 4, lx + 10, ly + 14], fill=c)
        d.text((lx + 15, ly), k.lower(), font=MONO_SMALL, fill=CODE_DIM)
        lx += 15 + d.textlength(k.lower(), font=MONO_SMALL) + 18


def fit(img, box_w, box_h):
    s = min(box_w / img.width, box_h / img.height)
    return img.resize((int(img.width * s), int(img.height * s)), Image.LANCZOS)


def result_text(shot):
    texts = [o for o in shot["outputs"] if not o.startswith(("rendered", "wrote", "saved", "hillshade over"))]
    return texts


def frame(shot, index, total, typed_chars, cursor, alpha, cache):
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    draw_panel(d, shot, typed_chars, cursor)
    x0 = 590
    d.text((x0, 22), f"{index}/{total}", font=SANS_SMALL, fill=MUTED)
    d.text((x0, 42), shot["title"], font=SANS_BOLD, fill=INK)
    d.text((x0, 78), shot["note"], font=SANS_SMALL, fill=MUTED)
    if alpha > 0:
        layer = Image.new("RGB", (W - x0, H - 110), BG)
        ld = ImageDraw.Draw(layer)
        y = 0
        texts = result_text(shot)
        if texts:
            for t in texts:
                for line in t.splitlines()[:9]:
                    ld.text((0, y), line[:74], font=MONO_SMALL, fill=INK)
                    y += 17
                y += 8
        if shot["image"]:
            key = shot["image"]
            if key not in cache:
                cache[key] = Image.open(key).convert("RGB")
            pic = fit(cache[key], W - x0 - 30, H - 110 - y - 70)
            layer.paste(pic, (0, y + 4))
        # timing: the executing steps
        slow = [l for l in shot["trace"] if not re.match(r"^\s*[0-9]\.\d ms", l)]
        t = "  ·  ".join(f"{m.group(2).lower()} {m.group(3).split()[0]} {float(m.group(1)):.0f} ms"
                         for l in slow for m in [re.match(r"^\s*([\d.]+) ms  (\w+)  (.*)$", l)] if m)
        ld.text((0, H - 110 - 28), t[:110], font=SANS_SMALL, fill=MUTED)
        base = im.crop((x0, 110, W, H))
        im.paste(Image.blend(base, layer, alpha), (x0, 110))
    return im


def stage2():
    shots_ = json.loads((OUT / "shots.json").read_text())
    FRAMES.mkdir(parents=True, exist_ok=True)
    for f in FRAMES.glob("*.png"):
        f.unlink()
    cache, n = {}, 0
    for i, shot in enumerate(shots_, 1):
        new_chars = sum(len(shot["steps"][j]) for j in new_indices(shot))
        typing = max(12, min(int(FPS * 1.6), new_chars // 3))
        hold, fade = int(FPS * 2.6), int(FPS * 0.35)
        for k in range(typing):
            chars = int(new_chars * (k + 1) / typing)
            frame(shot, i, len(shots_), chars, (k // 8) % 2 == 0, 0, cache).save(FRAMES / f"frame_{n:05d}.png")
            n += 1
        for k in range(fade + hold):
            frame(shot, i, len(shots_), 10**6, False, min(1.0, (k + 1) / fade), cache).save(FRAMES / f"frame_{n:05d}.png")
            n += 1
    print(f"{n} frames ({n / FPS:.1f} s)")


if __name__ == "__main__":
    import sys
    if "--frames-only" not in sys.argv:
        stage1()
    stage2()
