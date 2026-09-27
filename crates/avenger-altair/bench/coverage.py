"""Which of Altair's gallery examples Avenger takes, and what stops the rest.

    python bench/coverage.py ~/vega/altair/tests/examples_methods_syntax

For each example: run it, take its Vega-Lite (`to_dict(validate=False)`),
and ask Avenger. `spec` refusals are properties Avenger's Vega-Lite types do
not read; `compile` ones are read but not drawn yet. Prints a summary and
the refusals grouped by the first two parts of the path.
"""

import collections
import json
import sys
import time
from pathlib import Path

import altair as alt
from altair.utils.execeval import eval_block

import avenger_altair as av
from avenger_altair import _native

alt.data_transformers.disable_max_rows()
root = Path(sys.argv[1])
rows = []
configs = []
for f in sorted(root.glob("*.py")):
    if f.name.startswith("_"):
        continue
    try:
        chart = eval_block(f.read_text(), filename=str(f))
        spec = chart.to_dict(validate=False)
    except Exception as e:  # data that needs the network, or an example error
        rows.append((f.stem, "as written", "not run", type(e).__name__, ""))
        continue
    spec = av.normalise(spec)
    configs.append(json.dumps(spec.get("config"), sort_keys=True))
    # What stops it as Altair writes it, and what stops it once `config` is
    # set aside: the first says whether it is drawn, the second what the
    # grammar still lacks.
    for label, sp in (("as written", spec), ("without config", {k: v for k, v in spec.items() if k != "config"})):
        text = json.dumps(sp)
        r = _native.validate(text)
        if r is None:
            _, r = _native.render(text, "png", 1.0, str(root))
        if r is None:
            rows.append((f.stem, label, "drawn", "", ""))
        else:
            rows.append((f.stem, label, r["stage"], r["path"], r["message"]))

for label in ("as written", "without config"):
    sub = [r for r in rows if r[1] == label]
    count = collections.Counter(r[2] for r in sub)
    print(f"{label}: {len(sub)} examples: " + ", ".join(f"{n} {k}" for k, n in count.most_common()))
    for r in sub:
        if r[2] == "drawn":
            print("  drawn:", r[0])
    by = collections.Counter(".".join(r[3].split(".")[:2]) for r in sub if r[2] in ("spec", "compile"))
    print("  first refusal, by property:")
    for k, n in by.most_common(20):
        print(f"    {n:4d}  {k}")
print("config values, most common:")
for k, n in collections.Counter(configs).most_common(6):
    print(f"  {n:4d}  {k[:150]}")
json.dump(rows, open(Path(__file__).with_name("coverage.json"), "w"), indent=1)
