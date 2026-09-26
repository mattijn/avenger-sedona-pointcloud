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

# Altair calls the mark-mixin generator with Vega-Lite's names (Mark, BoxPlot,
# ErrorBar, ErrorBand); Avenger's are MarkType and BarMark.
marks = {"MarkType": "BarMark"}


def camera_mixin(fp: Path) -> str:
    """`camera_<type>()` per Camera variant in the schema, after Altair's
    MARK_METHOD: new grammar in Avenger becomes a method here."""
    root = json.loads(fp.read_text())
    code = ["class CameraMethodMixin:", '    """Methods that set how the plot is seen (Avenger\'s `camera`)."""']
    for variant in root["definitions"]["Camera"]["anyOf"]:
        kind = variant["properties"]["type"]["const"]
        args = [p for p in variant["properties"] if p != "type"]
        sig = "".join(f", {a}: Any = Undefined" for a in args)
        kw = ", ".join(f"{a}={a}" for a in args)
        doc = (variant.get("description") or f"A {kind} camera.").splitlines()[0]
        code.append(f"""
    def camera_{kind}(self{sig}) -> Self:
        \"\"\"{doc}\"\"\"
        copy = self.copy(deep=False)
        copy.camera = core.Camera(type="{kind}"{", " + kw if kw else ""})
        return copy""")
    return "\n".join(code)


mixins = []
for name, step in (
    ("core", g.generate_vegalite_schema_wrapper),
    ("channels", g.generate_vegalite_channel_wrappers),
    ("mark mixin", lambda f: g.generate_vegalite_mark_mixin(f, marks)),
    ("config mixin", g.generate_vegalite_config_mixin),
    ("camera mixin", camera_mixin),
):
    try:
        m = step(fp)
        code = m if isinstance(m, str) else (m.contents if isinstance(m.contents, str) else "\n".join(m.contents))
        if name in ("core", "channels"):
            (pkg / f"{name}.py").write_text(code)
        else:
            mixins.append(code)
        methods = [l.split("def ")[1].split("(")[0] for l in code.splitlines() if l.strip().startswith("def ") and not l.strip().startswith("def _")]
        print(f"{name}: {code.count(chr(10) + 'class ')} classes" + (f", methods {methods}" if name.endswith("mixin") else ""))
    except Exception as e:
        print(f"{name}: not generated ({type(e).__name__}: {str(e)[:90]})")
(pkg / "mixins.py").write_text(
    "from __future__ import annotations\n"
    "from typing import Any, Literal, Union\n"
    "from typing import Self\n"
    "from altair.utils import use_signature, Undefined, SchemaBase\n"
    "from . import core\n\n" + "\n\n".join(mixins) + "\n"
)

# A small Chart: the generated top-level class with the generated mixins, and
# an encode() over the generated channels. (Altair's own api.py does much more.)
(pkg / "api.py").write_text("""
from altair.utils import Undefined
from . import channels, core, mixins

_CHANNELS = {"x": channels.X, "y": channels.Y, "x2": channels.X2, "y2": channels.Y2}


class Chart(mixins.MarkMethodMixin, mixins.ConfigMethodMixin, mixins.CameraMethodMixin, core.TopLevelUnitSpec):
    def __init__(self, data=Undefined, mark="bar", **kwds):
        if hasattr(data, "to_dict") and not isinstance(data, core.SchemaBase):
            data = core.Data({"values": data.to_dict("records")})
        super().__init__(data=data, mark=mark, **kwds)

    def encode(self, **kwds):
        copy = self.copy(deep=False)
        enc = {k: (_CHANNELS[k](v) if isinstance(v, str) else v) for k, v in kwds.items()}
        copy.encoding = core.Encoding(**enc)
        return copy

    def properties(self, **kwds):
        copy = self.copy(deep=False)
        for k, v in kwds.items():
            setattr(copy, k, v)
        return copy
""")

import altair  # noqa: E402

shutil.copy(Path(altair.__file__).parent / "vegalite/v6/schema/_typing.py", pkg / "_typing.py")
(pkg / "__init__.py").write_text("from . import core, channels, mixins\nfrom .api import Chart\n")
sys.path.insert(0, str(out))
from avenger_alt import Chart, channels, core  # noqa: E402
import pandas as pd  # noqa: E402

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
# The same chart through the generated mixins, as one would write it in Altair.
df = pd.DataFrame(rows)
fluent = (
    Chart(df)
    .mark_bar(opacity=0.9)
    .encode(x="category:N", y=channels.Y("sum(amount):Q", title="total"))
    .properties(height=300)
    .configure_view(stroke=None)
)
spec2 = fluent.to_dict()
png2, refusal2 = _native.render(json.dumps(spec2), "png", 2.0, None)
(out / "bars_fluent.png").write_bytes(png2 or b"")
print("Chart(df).mark_bar().encode(...).configure_view(...):", "drawn by Avenger" if refusal2 is None else refusal2, "| keys", sorted(spec2))
cam = Chart(df).mark_bar().encode(x="category:N", y="amount:Q").camera_fisheye(focus=[0.3, 0.5], radius=0.4)
print("camera_fisheye ->", cam.to_dict()["camera"])
try:
    Chart(df).mark_bar().camera_tilt(elevation=120).to_dict()
except Exception as e:
    print("camera_tilt(elevation=120):", str(e).splitlines()[0])
t = time.perf_counter()
for _ in range(200):
    chart.to_dict()
print(f"to_dict with validation: {(time.perf_counter() - t) / 200 * 1e6:.0f} µs")
