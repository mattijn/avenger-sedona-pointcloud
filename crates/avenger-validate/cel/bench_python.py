"""The Rust validator from Python: verdicts and time on experiment 7's
corpus. `python bench_python.py <corpus.jsonl> <pipelines.txt>`"""

import json
import sys
import time

import avenger_validate as av

texts = [json.loads(l) for l in open(sys.argv[1])]
t = time.perf_counter()
v = av.Validator()
start = (time.perf_counter() - t) * 1e3
pipes = [json.loads(l) for l in open(sys.argv[2])]
rs = v.check_many([p["pipeline"] for p in pipes])
caught = sum(1 for p, r in zip(pipes, rs) if p["refused"] and not r["valid"])
false_pos = sum(1 for p, r in zip(pipes, rs) if not p["refused"] and not r["valid"])
print(f"start-up {start:.1f} ms; refused and flagged {caught} of {sum(p['refused'] for p in pipes)}; accepted but flagged {false_pos}")
for label, fn in [("one call a pipeline", lambda: [v.check(p["pipeline"]) for p in pipes]), ("one call for all", lambda: v.check_many([p["pipeline"] for p in pipes]))]:
    n = 20
    t = time.perf_counter()
    for _ in range(n):
        fn()
    print(f"{label}: {(time.perf_counter() - t) / n / len(pipes) * 1e6:.1f} µs a pipeline (warm)")
