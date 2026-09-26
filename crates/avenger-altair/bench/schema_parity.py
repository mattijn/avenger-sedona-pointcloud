"""Does the portable validator (JSON Schema + CEL, no Rust) accept exactly
what Avenger's Rust types accept?

    python bench/schema_parity.py [avenger checkout] [altair gallery dir]

Seeds: Avenger's own fixtures and examples, and Altair bar charts. Each seed
is mutated systematically (an unknown key on every object, null, wrong
types and out-of-range numbers on every property, removed properties, and
each rule broken on purpose); every spec is then judged by both.
"""

import copy
import json
import sys
import time
from pathlib import Path

import altair as alt
import pandas as pd

import avenger_altair as av
from avenger_altair import _native, portable

avenger = Path(sys.argv[1] if len(sys.argv) > 1 else "../avenger").expanduser()
seeds = []
for p in list(avenger.glob("avenger-vegalite-spec/tests/fixtures/*.json")) + list(avenger.glob("avenger-vegalite-compiler/examples/*.json")) + list(avenger.glob("avenger-vegalite-compiler/tests/fixtures/*.json")):
    try:
        s = json.loads(p.read_text())
    except Exception:
        continue
    if isinstance(s, dict) and "mark" in s:
        seeds.append((p.name, s))
    elif isinstance(s, dict):
        for k, v in s.items():
            if isinstance(v, dict) and "mark" in v:
                seeds.append((f"{p.name}:{k}", v))
df = pd.DataFrame({"category": list("ABCD"), "amount": [3, 5, 2, 8], "delay": [1.0, 2.5, -3.0, 4.0]})
charts = {
    "altair bars": alt.Chart(df).mark_bar().encode(x="category:N", y="amount:Q"),
    "altair sum": alt.Chart(df).mark_bar(opacity=0.8).encode(x=alt.X("category:N", sort="descending", axis=alt.Axis(labelAngle=-45)), y=alt.Y("sum(amount):Q", title="total")),
    "altair histogram": alt.Chart(df).mark_bar().encode(alt.X("delay:Q", bin=alt.Bin(maxbins=20, extent=[-5, 5], steps=[1, 2, 5])), y="count()"),
    "altair horizontal": alt.Chart(df).mark_bar().encode(y="category:N", x=alt.X("amount:Q", scale=alt.Scale(zero=False, nice=True))),
    "altair params": alt.Chart(df).mark_bar().encode(x="category:N", y="amount:Q").transform_filter(alt.FieldGTEPredicate(field="amount", gte=2)).properties(width=200, height=100, title=["a", "b"]),
    "altair bin transform": alt.Chart(df).mark_bar().encode(x="bin_lo:Q", x2="bin_hi:Q", y="count()").transform_bin(["bin_lo", "bin_hi"], "delay", bin=alt.Bin(step=1)),
    "altair aggregate": alt.Chart(df).mark_bar().encode(x="category:N", y="total:Q").transform_aggregate(total="sum(amount)", groupby=["category"]),
}
for name, c in charts.items():
    seeds.append((name, av.normalise(c.to_dict(validate=False))))


def nodes(obj, path=()):
    yield path, obj
    if isinstance(obj, dict):
        for k, v in obj.items():
            yield from nodes(v, path + (k,))
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            yield from nodes(v, path + (i,))


def at(obj, path):
    for p in path:
        obj = obj[p]
    return obj


def mutants(spec):
    for path, node in list(nodes(spec)):
        if path and path[0] in ("datasets", "usermeta") or (len(path) >= 2 and path[:2] == ("data", "values")):
            continue  # free-form rows and metadata
        if isinstance(node, dict):
            m = copy.deepcopy(spec); at(m, path)["zzz_unknown"] = 1; yield f"unknown key at {path}", m
            for k in list(node):
                m = copy.deepcopy(spec); del at(m, path)[k]; yield f"remove {path + (k,)}", m
        if path:
            parent, key = path[:-1], path[-1]
            for label, value in (("null", None), ("string", "zz"), ("number", 3), ("negative", -1), ("zero", 0), ("half", 0.5), ("big", 400), ("bool", True), ("empty list", []), ("empty object", {})):
                if value == node and type(value) is type(node):
                    continue
                m = copy.deepcopy(spec); at(m, parent)[key] = value; yield f"{label} at {path}", m
    # Each rule broken on purpose.
    enc = spec.get("encoding", {})
    for ch in ("x", "y"):
        b = (enc.get(ch) or {}).get("bin")
        if isinstance(b, dict):
            for label, change in (("extent reversed", {"extent": [5, -5]}), ("steps not increasing", {"steps": [5, 2]}), ("steps equal", {"steps": [2, 2]}), ("steps increasing", {"steps": [1, 2, 3]})):
                m = copy.deepcopy(spec); m["encoding"][ch]["bin"].update(change); yield label, m
    m = copy.deepcopy(spec); m["params"] = [{"name": "a", "value": 1}, {"name": "a", "value": 2}]; yield "duplicate param names", m
    m = copy.deepcopy(spec); m["params"] = [{"name": "a", "value": 1}, {"name": "b", "value": 2}]; yield "distinct param names", m
    m = copy.deepcopy(spec); m["params"] = [{"name": "datum", "value": 1}]; yield "reserved param name", m
    m = copy.deepcopy(spec); m["params"] = [{"name": "1a", "value": 1}]; yield "param name pattern", m
    m = copy.deepcopy(spec); m.setdefault("transform", []).append({"aggregate": [{"op": "sum", "field": "a", "as": "t"}, {"op": "mean", "field": "b", "as": "t"}], "groupby": ["c"]}); yield "duplicate aggregate aliases", m
    m = copy.deepcopy(spec); m.setdefault("transform", []).append({"aggregate": [{"op": "count", "as": "n"}]}); yield "count without field", m
    m = copy.deepcopy(spec); m.setdefault("transform", []).append({"bin": True, "field": "a", "as": ["lo", "lo"]}); yield "equal bin aliases", m
    m = copy.deepcopy(spec); m.setdefault("transform", []).append({"bin": False, "field": "a", "as": "lo"}); yield "bin transform false", m
    m = copy.deepcopy(spec); m.setdefault("transform", []).append({"bin": True, "aggregate": [{"op": "count", "as": "n"}], "field": "a", "as": "lo"}); yield "bin and aggregate", m


rows, t_rust, t_port = [], 0.0, 0.0
for name, seed in seeds:
    for label, spec in [("seed", seed)] + list(mutants(seed)):
        text = json.dumps(spec)
        t = time.perf_counter(); rust = _native.validate(text); t_rust += time.perf_counter() - t
        t = time.perf_counter(); port = portable.validate(spec); t_port += time.perf_counter() - t
        rows.append((name, label, rust is None, not port, rust, port))

agree = sum(1 for r in rows if r[2] == r[3])
print(f"{len(seeds)} seeds, {len(rows)} specs: {agree} agree ({100 * agree / len(rows):.2f} %), "
      f"{sum(r[2] for r in rows)} valid for Rust, {sum(r[3] for r in rows)} valid for the portable validator")
print(f"mean per spec: Rust (from Python) {t_rust / len(rows) * 1e6:.1f} µs, JSON Schema + CEL (Python) {t_port / len(rows) * 1e6:.0f} µs")
for name, label, r_ok, p_ok, rust, port in rows:
    if r_ok != p_ok:
        print(f"  DIFFERS {name} | {label} | Rust {'valid' if r_ok else rust['path'] + ': ' + rust['message'][:70]} | portable {'valid' if p_ok else port[:2]}")
