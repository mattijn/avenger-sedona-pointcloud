"""The smallest materialisation budget that draws a histogram over N rows,
with the table as JSON rows and as Arrow (FINDINGS.md 22).

    python bench/budget.py 30000 100000 300000 1000000
"""

import json
import os
import sys

import altair as alt
import numpy as np
import pandas as pd

import avenger_altair as av
from avenger_altair import _native

alt.data_transformers.disable_max_rows()


def smallest(render) -> int:
    lo, hi = 1 << 16, 64 << 30
    while hi / lo > 1.05:
        mid = int((lo * hi) ** 0.5)
        os.environ["AVENGER_MAX_MATERIALIZED_BYTES"] = str(mid)
        if render() is None:
            hi = mid
        else:
            lo = mid
    return hi


for n in [int(a) for a in sys.argv[1:]] or [100_000, 1_000_000]:
    df = pd.DataFrame({"value": np.random.default_rng(0).normal(0, 1, n)})
    chart = lambda: alt.Chart(df).mark_bar().encode(alt.X("value:Q", bin=alt.Bin(maxbins=20)), y="count()")
    rows = json.dumps(av.normalise(chart().to_dict(validate=False)))
    av.enable(validation=False)
    spec = av.normalise(chart().to_dict(validate=False))
    arrow = lambda: _native.render(json.dumps(spec), "png", 1.0, None, av._bound(spec))[1]
    a = smallest(arrow)
    av.disable()
    alt.data_transformers.disable_max_rows()
    j = smallest(lambda: _native.render(rows, "png", 1.0, None)[1])
    print(f"n={n}: JSON rows {j / 1e6:.1f} MB ({j / n:.0f} B/row), Arrow {a / 1e6:.1f} MB ({a / n:.0f} B/row)")
