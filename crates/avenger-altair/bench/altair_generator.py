"""Feed Avenger's schema to Altair's own generator, and use what comes out.

    python bench/altair_generator.py <altair checkout> <output dir>

Reads the checkout (tools/generate_schema_wrapper.py) and changes nothing in
it. Writes a package `avenger_alt` (core.py, channels.py) to the output
directory, then builds a bar chart with the generated classes, validates it
with them, and has Avenger validate and draw it.
"""

import json
import shutil
import sys
import time
from pathlib import Path

altair_checkout, out = Path(sys.argv[1]).expanduser(), Path(sys.argv[2])
here = Path(__file__).resolve().parent
schema = json.loads((here.parent / "python/avenger_altair/avenger-vegalite.schema.json").read_text())

# The shape the generator reads: draft-07 `definitions`, the root as a
# definition, and the two names it looks up.
s = json.loads(json.dumps(schema).replace('"#/$defs/', '"#/definitions/'))
s["definitions"] = s.pop("$defs")
s["definitions"]["TopLevelUnitSpec"] = {k: v for k, v in s.items() if k not in ("definitions", "$schema", "title")}
s["definitions"]["FacetedEncoding"] = s["definitions"]["Encoding"]
s["definitions"]["RepeatRef"] = {"type": "object", "properties": {"repeat": {"type": "string"}}, "required": ["repeat"], "additionalProperties": False}
pkg = out / "avenger_alt"
pkg.mkdir(parents=True, exist_ok=True)
fp = pkg / "vega-lite-schema.json"
fp.write_text(json.dumps(s, indent=1))

sys.path.insert(0, str(altair_checkout))
from tools import generate_schema_wrapper as g  # noqa: E402

marks = {k: f"{k}Def" for k in ["Mark", "BoxPlot", "ErrorBar", "ErrorBand"]}
for name, step in (
    ("core", g.generate_vegalite_schema_wrapper),
    ("channels", g.generate_vegalite_channel_wrappers),
    ("mark mixin", lambda f: g.generate_vegalite_mark_mixin(f, marks)),
    ("config mixin", g.generate_vegalite_config_mixin),
):
    try:
        m = step(fp)
        code = m if isinstance(m, str) else (m.contents if isinstance(m.contents, str) else "\n".join(m.contents))
        if name in ("core", "channels"):
            (pkg / f"{name}.py").write_text(code)
        print(f"{name}: {code.count(chr(10) + 'class ')} classes")
    except Exception as e:
        print(f"{name}: not generated ({type(e).__name__}: {str(e)[:90]})")

import altair  # noqa: E402

shutil.copy(Path(altair.__file__).parent / "vegalite/v6/schema/_typing.py", pkg / "_typing.py")
(pkg / "__init__.py").write_text("from . import core, channels\n")
sys.path.insert(0, str(out))
from avenger_alt import channels, core  # noqa: E402

from avenger_altair import _native  # noqa: E402

rows = [{"category": c, "amount": a} for c, a in zip("ABCDEFGH", [3, 5, 2, 8, 4, 6, 7, 1])]
chart = core.TopLevelUnitSpec(
    data=core.Data({"values": rows}),
    mark="bar",
    encoding=core.Encoding(x=channels.X("category:N"), y=channels.Y("sum(amount):Q", title="total")),
    height=300,
)
spec = chart.to_dict()
png, refusal = _native.render(json.dumps(spec), "png", 2.0, None)
(out / "bars.png").write_bytes(png or b"")
print("a chart from the generated classes:", "drawn by Avenger" if refusal is None else refusal)
for label, make in (
    ("mark point", lambda: core.TopLevelUnitSpec(data=core.Data({"values": rows}), mark="point").to_dict()),
    ("maxbins 1", lambda: core.TopLevelUnitSpec(data=core.Data({"values": rows}), mark="bar", encoding=core.Encoding(x=channels.X("amount:Q", bin=core.BinParams(maxbins=1)))).to_dict()),
):
    try:
        make()
        print(f"{label}: accepted")
    except Exception as e:
        print(f"{label}: {str(e).splitlines()[0]}")
t = time.perf_counter()
for _ in range(200):
    chart.to_dict()
print(f"to_dict with validation: {(time.perf_counter() - t) / 200 * 1e6:.0f} µs")
