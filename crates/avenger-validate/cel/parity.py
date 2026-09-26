"""Check that cel-python and cel-js give what the Rust validator gives, on
every pipeline of experiment 7: `(layer, step)` of each layer 1-2 issue.

    avenger-validate export-cel > /tmp/bundle.json
    avenger-validate corpus <results dir> --jsonl > /tmp/corpus.jsonl
    python parity.py /tmp/bundle.json /tmp/corpus.jsonl
"""

import json
import subprocess
import sys
import time
from pathlib import Path

from avenger_validate_cel import Bundle

bundle_path, corpus_path = sys.argv[1], sys.argv[2]
rows = [json.loads(line) for line in open(corpus_path)]
want = [[(i["layer"], i["step"]) for i in r["issues"]] for r in rows]

b = Bundle(json.load(open(bundle_path)))
t = time.perf_counter()
got = [[(i["layer"], i["step"]) for i in b.validate(r["steps"])] for r in rows]
py_us = (time.perf_counter() - t) / len(rows) * 1e6
bad = [k for k, (w, g) in enumerate(zip(want, got)) if w != g]
t = time.perf_counter()
for r in rows:
    b.validate(r["steps"])
warm_us = (time.perf_counter() - t) / len(rows) * 1e6
print(f"cel-python: {len(rows) - len(bad)} of {len(rows)} agree with Rust ({py_us:.0f} µs a pipeline first pass, {warm_us:.0f} µs warm)")
for k in bad[:5]:
    print("  differs:", rows[k]["steps"], want[k], got[k])

js = Path(__file__).with_name("parity.mjs")
out = subprocess.run(["node", str(js), bundle_path, corpus_path], capture_output=True, text=True, check=True).stdout
print(out.strip())
