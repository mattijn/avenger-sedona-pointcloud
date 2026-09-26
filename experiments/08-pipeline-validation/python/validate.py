"""The same four-layer validator as `bench/src/main.rs`, in pure Python, from the
same `spec/steps.json`, over the same corpus: every pipeline experiment 7's
writers produced. Layer 3 is left out: there is no Python parser for Vega
expressions.

    pip install cel-python datafusion==54.* pyarrow
    python validate.py [repeats]
"""

import json
import sys
import time
from pathlib import Path

import celpy
import pyarrow as pa
from datafusion import SessionContext

HERE = Path(__file__).resolve().parent


def parse_pipeline(src):
    """A port of experiment 6's `parse_pipeline`."""
    words, cur, quoted, in_quote, i = [], "", False, None, 0
    while i < len(src):
        c = src[i]
        if in_quote:
            if c == in_quote:
                in_quote = None
            elif c == "\\":
                i += 1
                if i < len(src):
                    cur += src[i]
            else:
                cur += c
        elif c in "\"'":
            in_quote, quoted = c, True
        elif c.isspace():
            if cur or quoted:
                words.append((cur, quoted))
                cur = ""
            quoted = False
        else:
            cur += c
        i += 1
    if in_quote:
        raise ValueError("unterminated quote in pipeline")
    if cur or quoted:
        words.append((cur, quoted))
    groups, g = [], []
    for w in words:
        if w == ("!", False):
            groups.append(g)
            g = []
        else:
            g.append(w)
    groups.append(g)
    calls = []
    for g in groups:
        if not g:
            continue
        name, args, flags, j = g[0][0], [], {}, 1
        while j < len(g):
            w, q = g[j]
            if w.startswith("--") and not q:
                if j + 1 < len(g) and (g[j + 1][1] or not g[j + 1][0].startswith("--")):
                    flags[w[2:]] = g[j + 1][0]
                    j += 1
                else:
                    flags[w[2:]] = "true"
            else:
                args.append(w)
            j += 1
        calls.append((name, args, flags))
    return calls


def is_hex(v):
    return len(v) == 7 and v[0] == "#" and all(c in "0123456789abcdefABCDEF" for c in v[1:])


def number(v):
    try:
        return float(v)
    except ValueError:
        return None


def unit(v):
    x = number(v)
    return x is not None and 0 <= x <= 1


def check_type(ty, v):
    if isinstance(ty, list):
        return v in ty
    if ty == "hex":
        return is_hex(v)
    if ty == "unit":
        return unit(v)
    if ty == "unit_pair":
        p = v.split(",")
        return len(p) == 2 and all(unit(s) for s in p)
    if ty == "number":
        return number(v) is not None
    if ty == "flag":
        return v in ("true", "false")
    if ty == "channel":
        if is_hex(v):
            return True
        f, _, t = v.partition(":")
        return bool(f) and (t == "" or t in "NOQT" and len(t) == 1)
    return True


LAS = pa.schema([
    ("x", pa.float64()), ("y", pa.float64()), ("z", pa.float64()),
    ("intensity", pa.uint16()), ("return_number", pa.uint8()), ("number_of_returns", pa.uint8()),
    ("classification", pa.uint8()), ("scan_angle", pa.float32()), ("user_data", pa.uint8()),
    ("point_source_id", pa.uint16()), ("gps_time", pa.float64()),
    ("red", pa.uint16()), ("green", pa.uint16()), ("blue", pa.uint16()),
])


def set_input(ctx, schema):
    ctx.deregister_table("input")
    ctx.register_record_batches("input", [[pa.RecordBatch.from_pylist([], schema=schema)]])


def load_spec(path):
    raw = json.loads(Path(path).read_text())
    env = celpy.Environment()
    for s in raw["steps"].values():
        s["program"] = env.program(env.compile(s.get("requires", "true")))
    return raw


def validate(spec, ctx, src, t):
    t0 = time.perf_counter()
    try:
        calls = parse_pipeline(src)
    except ValueError as e:
        return [f"L0 {e}"]
    t["parse"] += time.perf_counter() - t0
    errs, state, schema = [], dict(spec["state"]), None
    for i, (name, pos, flags) in enumerate(calls):
        step = spec["steps"].get(name)
        if step is None:
            errs.append(f"L1 step {i}: unknown step `{name}`")
            continue
        t1 = time.perf_counter()
        args = {}
        for (pname, ty), v in zip(step.get("positional", []), pos):
            if not check_type(ty, v):
                errs.append(f"L1 {name} {pname}: `{v}` is not a {ty}")
            args[pname] = v
        for k, v in flags.items():
            ty = step.get("flags", {}).get(k)
            if ty is None:
                errs.append(f"L1 {name}: no flag --{k}")
                continue
            if not check_type(ty, v):
                errs.append(f"L1 {name} --{k}: `{v}` is not a {ty}")
            args[k] = v
        t2 = time.perf_counter()
        t["l1"] += t2 - t1
        ok = step["program"].evaluate({"state": celpy.json_to_cel(state), "step": celpy.json_to_cel(args)})
        if ok is not True and ok != celpy.celtypes.BoolType(True):
            errs.append(f"L2 {name}: needs {step.get('requires')} (state {state})")
        for k, v in step.get("sets", {}).items():
            state[k] = args.get(v[1:], "") if v.startswith("$") else v
        t3 = time.perf_counter()
        t["l2"] += t3 - t2
        if name == "read":
            schema = LAS
            set_input(ctx, LAS)
        if name == "sql" and "query" in args:
            try:
                schema = ctx.sql(args["query"]).schema()
                set_input(ctx, schema)
            except Exception as e:
                errs.append(f"L4 sql: {str(e).splitlines()[0]}")
        if schema is not None:
            types = dict(step.get("positional", []))
            types.update(step.get("flags", {}))
            for a, ty in types.items():
                v = args.get(a)
                if ty == "channel" and v and not is_hex(v):
                    f = v.split(":")[0]
                    if f not in schema.names:
                        errs.append(f"L4 {name} --{a}: no field `{f}`")
        t["l4"] += time.perf_counter() - t3
    return errs


def corpus(d):
    out = []
    for f in sorted(Path(d).glob("*decisions.json")):
        for x in json.loads(f.read_text()):
            for route in x.values():
                if isinstance(route, dict):
                    for a in route.get("attempts", []):
                        if a.get("pipeline"):
                            out.append((a["pipeline"], a.get("refused") is not None))
    return out


def main():
    repeats = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    t0 = time.perf_counter()
    spec = load_spec(HERE.parent / "spec" / "steps.json")
    load = time.perf_counter() - t0
    t0 = time.perf_counter()
    ctx = SessionContext()
    session = time.perf_counter() - t0
    pipelines = corpus(HERE.parent.parent / "07-chart-decisions" / "results")
    agree, by_layer, dummy = {}, {}, dict.fromkeys(["parse", "l1", "l2", "l4"], 0.0)
    for p, refused in pipelines:
        errs = validate(spec, ctx, p, dummy)
        key = (refused, bool(errs))
        agree[key] = agree.get(key, 0) + 1
        for e in errs:
            by_layer[e[:2]] = by_layer.get(e[:2], 0) + 1
    t = dict.fromkeys(["parse", "l1", "l2", "l4"], 0.0)
    wall = time.perf_counter()
    for _ in range(repeats):
        for p, _ in pipelines:
            validate(spec, ctx, p, t)
    wall = time.perf_counter() - wall
    n = repeats * len(pipelines)
    us = lambda s: s * 1e6 / n
    print(f"python: {len(pipelines)} pipelines x {repeats} repeats")
    print(f"  load spec + compile CEL (once) {load * 1e6:>9.1f} µs")
    print(f"  SessionContext (once)          {session * 1e6:>9.1f} µs")
    print("  per pipeline, mean:")
    print(f"    L0 parse             {us(t['parse']):>9.2f} µs")
    print(f"    L1 arguments         {us(t['l1']):>9.2f} µs")
    print(f"    L2 order and state   {us(t['l2']):>9.2f} µs")
    print("    L3 Vega expressions        n/a")
    print(f"    L4 SQL and fields    {us(t['l4']):>9.2f} µs")
    print(f"    total (wall)         {wall * 1e6 / n:>9.2f} µs")
    print(f"  verdicts (recorded refused, flagged here): {dict(sorted(agree.items()))}")
    print(f"  errors by layer: {dict(sorted(by_layer.items()))}")


if __name__ == "__main__":
    main()
