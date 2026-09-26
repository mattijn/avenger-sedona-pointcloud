"""Layers 1 and 2 of avenger-validate, from its CEL export, with cel-python.

    pip install cel-python
    avenger-validate export-cel > bundle.json
    python avenger_validate_cel.py bundle.json steps.json

`steps.json` is a pipeline as data: [{"step": "chart", "args": ["bar"],
"flags": {"x": "label:N"}}, ...], all values strings. No Rust is needed: the
bundle holds every rule of layers 1 and 2. Layers 3 (Vega expressions) and 4
(SQL against the schema) need the library.
"""

import json
import sys

import celpy
from celpy import celtypes


class Bundle:
    def __init__(self, bundle: dict):
        self.bundle = bundle
        self.env = celpy.Environment()
        self._programs = {}

    def _run(self, src: str, activation: dict) -> bool:
        prog = self._programs.get(src)
        if prog is None:
            prog = self._programs[src] = self.env.program(self.env.compile(src))
        try:
            return prog.evaluate(activation) == celtypes.BoolType(True)
        except Exception:
            return False

    def _check(self, src: str, v: str) -> bool:
        return self._run(src, {"v": celtypes.StringType(v)})

    def validate(self, steps: list) -> list:
        """Issues as dicts: layer, step, arg, message."""
        issues = []
        state = dict(self.bundle["state"])
        for i, s in enumerate(steps):
            spec = self.bundle["steps"].get(s["step"])
            if spec is None:
                issues.append({"layer": 1, "step": i, "arg": None, "message": f"unknown step `{s['step']}`"})
                continue
            args, given = {}, list(s.get("args", []))
            for k, p in enumerate(spec["positional"]):
                if k < len(given):
                    if not self._check(p["check"], given[k]):
                        issues.append({"layer": 1, "step": i, "arg": p["name"], "message": f"`{given[k]}` is not a {p['type']}"})
                    args[p["name"]] = given[k]
                elif not p["optional"]:
                    issues.append({"layer": 1, "step": i, "arg": p["name"], "message": f"needs its {p['name']}"})
            if len(given) > len(spec["positional"]):
                issues.append({"layer": 1, "step": i, "arg": None, "message": "too many positional arguments"})
            for k, v in s.get("flags", {}).items():
                f = spec["flags"].get(k)
                if f is None:
                    issues.append({"layer": 1, "step": i, "arg": f"--{k}", "message": f"no flag --{k}"})
                    continue
                if not self._check(f["check"], v):
                    issues.append({"layer": 1, "step": i, "arg": f"--{k}", "message": f"`{v}` is not a {f['type']}"})
                args[k] = v
            env = {"state": celpy.json_to_cel(state), "step": celpy.json_to_cel(args)}
            if not self._run(spec["requires"], env):
                issues.append({"layer": 2, "step": i, "arg": None, "message": spec.get("message") or f"needs {spec['requires']}"})
            for k, v in spec["sets"].items():
                state[k] = args.get(v[1:], "") if v.startswith("$") else v
        return issues


if __name__ == "__main__":
    bundle = Bundle(json.load(open(sys.argv[1])))
    issues = bundle.validate(json.load(open(sys.argv[2])))
    print(json.dumps(issues, indent=2))
    sys.exit(1 if issues else 0)
