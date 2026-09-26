"""A histogram over N values, drawn to PNG four ways, after
vega/altair#3035 (discussioncomment-10647136): where does the time go?

    python bench/large.py 10000 100000 1000000 3000000

Routes: Altair's default (inline JSON rows, vl-convert), Altair with the
VegaFusion data transformer (vl-convert), Altair with avenger-altair (per
stage), and matplotlib's `hist` as a floor.
"""

import io
import json
import sys
import time

import altair as alt
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
import vl_convert as vlc

import avenger_altair as av
from avenger_altair import _native

alt.data_transformers.disable_max_rows()
# LARGE_ALL=1 runs every route at every size (vl-convert over 3M takes a while).
ALL = bool(__import__("os").environ.get("LARGE_ALL"))


def chart(df):
    return alt.Chart(df).mark_bar().encode(alt.X("value:Q", bin=alt.Bin(maxbins=20)), y="count()")


def clock(f):
    t = time.perf_counter()
    r = f()
    return r, (time.perf_counter() - t) * 1e3


# Warm up: the first PNG initialises the GPU device, once per process.
_native.render(json.dumps(av.normalise(chart(pd.DataFrame({"value": [0.0, 1.0]})).to_dict(validate=False))), "png", 2.0, None)
vlc.vegalite_to_png(chart(pd.DataFrame({"value": [0.0, 1.0]})).to_dict(validate=False), scale=2)

rows = []
for n in [int(a) for a in sys.argv[1:]] or [10_000, 100_000, 1_000_000]:
    rng = np.random.default_rng(0)
    df = pd.DataFrame({"value": rng.normal(0, 1, n)})
    r = {"n": n}
    # Avenger, stage by stage.
    alt.data_transformers.enable("default")
    alt.data_transformers.disable_max_rows()
    spec, r["to_dict"] = clock(lambda: av.normalise(chart(df).to_dict(validate=False)))
    text, r["json.dumps"] = clock(lambda: json.dumps(spec))
    if n <= 1_000_000 or ALL:
        (png, refusal), r["avenger (Rust)"] = clock(lambda: _native.render(text, "png", 2.0, None))
        if refusal is not None:
            r["avenger (Rust)"] = float("nan")
            r["avenger refusal"] = refusal["message"]
    else:
        r["avenger (Rust)"] = float("nan")  # JSON rows exceed the default budget
    r["avenger total"] = r["to_dict"] + r["json.dumps"] + r["avenger (Rust)"]
    # Avenger with the Arrow data path: the frame goes by name.
    av.enable(arrow=True, validation=False)
    spec2, r["arrow: to_dict"] = clock(lambda: av.normalise(chart(df).to_dict(validate=False)))
    (png2, refusal2), r["arrow: avenger (Rust)"] = clock(lambda: _native.render(json.dumps(spec2), "png", 2.0, None, av._bound(spec2)))
    if refusal2 is not None:
        r["arrow: avenger (Rust)"] = float("nan")
        r["arrow refusal"] = refusal2["message"]
    r["arrow: avenger total"] = r["arrow: to_dict"] + r["arrow: avenger (Rust)"]
    av.disable()
    # Altair's default route.
    if n <= 1_000_000 or ALL:
        _, r["vl-convert total"] = clock(lambda: vlc.vegalite_to_png(chart(df).to_dict(validate=False), scale=2))
    # VegaFusion pre-evaluates the transforms, then vl-convert draws 20 bars.
    alt.data_transformers.enable("vegafusion")
    buf = io.BytesIO()
    _, r["vegafusion total"] = clock(lambda: chart(df).save(buf, format="png", scale_factor=2))
    alt.data_transformers.enable("default")
    # matplotlib.
    def mpl():
        fig, ax = plt.subplots(figsize=(3, 3))
        ax.hist(df["value"].to_numpy(), bins=20)
        b = io.BytesIO()
        fig.savefig(b, format="png", dpi=200)
        plt.close(fig)
    _, r["matplotlib total"] = clock(mpl)
    rows.append(r)
    print({k: (round(v, 1) if isinstance(v, float) else v) for k, v in r.items()}, flush=True)

open(sys.argv[0].replace("large.py", "large.json"), "w").write(json.dumps(rows, indent=1))
