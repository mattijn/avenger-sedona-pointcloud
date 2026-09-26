"""Time the Rust validator from Python, through its pyo3 module, over the same
corpus as `validate.py`. Build the module first, from `../bench`:

    maturin develop --release
    python bench_pyo3.py [repeats]
"""

import sys
import time
from pathlib import Path

import lidar_validate

from validate import corpus

HERE = Path(__file__).resolve().parent


def main():
    repeats = int(sys.argv[1]) if len(sys.argv) > 1 else 20
    t0 = time.perf_counter()
    v = lidar_validate.Validator(str(HERE.parent / "spec" / "steps.json"))
    load = time.perf_counter() - t0
    pipelines = corpus(HERE.parent.parent / "07-chart-decisions" / "results")
    texts = [p for p, _ in pipelines]
    agree = {}
    for (p, refused), errs in zip(pipelines, v.check_many(texts)):
        key = (refused, bool(errs))
        agree[key] = agree.get(key, 0) + 1

    t0 = time.perf_counter()
    for _ in range(repeats):
        for p in texts:
            v.check(p)
    one = time.perf_counter() - t0
    t0 = time.perf_counter()
    for _ in range(repeats):
        v.check_many(texts)
    many = time.perf_counter() - t0
    n = repeats * len(texts)
    print(f"pyo3: {len(texts)} pipelines x {repeats} repeats")
    print(f"  Validator() (once)            {load * 1e6:>9.1f} µs")
    print(f"  check(text), per pipeline     {one * 1e6 / n:>9.2f} µs")
    print(f"  check_many(texts), per pipeline {many * 1e6 / n:>7.2f} µs")
    print(f"  verdicts (recorded refused, flagged here): {dict(sorted(agree.items()))}")


if __name__ == "__main__":
    main()
