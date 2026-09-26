"""A histogram fed in batches: compile once, append, draw.

    python bench/live.py [batch rows] [batches]

Against drawing the whole table again each time (validate, compile, render).
"""

import json
import sys
import time

import altair as alt
import numpy as np
import pandas as pd

import avenger_altair as av
from avenger_altair import _native

batch, count = (int(a) for a in (sys.argv[1:3] if len(sys.argv) > 2 else (10_000, 100)))
rng = np.random.default_rng(0)
first = pd.DataFrame({"value": rng.normal(0, 1, batch)})
chart = alt.Chart(first).mark_bar().encode(alt.X("value:Q", bin=alt.Bin(maxbins=20)), y="count()")

t = time.perf_counter()
live = av.live(chart)
live.png()
print(f"compile and first draw: {(time.perf_counter() - t) * 1e3:.1f} ms, {live.rows:,} rows")
frames = [first]
for k in range(1, count):
    df = pd.DataFrame({"value": rng.normal(0, 1, batch)})
    frames.append(df)
    a = time.perf_counter(); live.append(df); append = time.perf_counter() - a
    d = time.perf_counter(); png = live.png(); draw = time.perf_counter() - d
    if k in (1, 9, 49, count - 1):
        # The same rows drawn from scratch: one frame, compiled again.
        whole = pd.concat(frames, ignore_index=True)
        av.enable(validation=False)
        spec = av.normalise(alt.Chart(whole).mark_bar().encode(alt.X("value:Q", bin=alt.Bin(maxbins=20)), y="count()").to_dict(validate=False))
        f = time.perf_counter(); _native.render(json.dumps(spec), "png", 2.0, None, av._bound(spec)); fresh = time.perf_counter() - f
        av.disable()
        print(f"{live.rows:>9,} rows: append {append * 1e3:5.2f} ms, draw {draw * 1e3:6.1f} ms  |  from scratch {fresh * 1e3:6.1f} ms")
open(sys.argv[0].replace("live.py", "live.png"), "wb").write(png)
