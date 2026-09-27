"""Validate a spec against Avenger's Vega-Lite subset without Rust.

The schema (`avenger-vegalite.schema.json`) is generated from Avenger's own
spec types. Structure, types and ranges are JSON Schema; what JSON Schema
cannot state is CEL over `self` in `x-avenger-rules`, run on every node its
subschema matches, as Kubernetes runs `x-kubernetes-validations`.

    from avenger_altair.portable import validate
    validate(spec)      # [] if Avenger takes it, else [(path, message), ...]

Needs `jsonschema` and `cel-python`; no compiled code.
"""

from __future__ import annotations

import json
from functools import lru_cache
from pathlib import Path

import jsonschema
from jsonschema import validators

SCHEMA_PATH = Path(__file__).with_name("avenger-vegalite.schema.json")


@lru_cache(maxsize=None)
def schema() -> dict:
    return json.loads(SCHEMA_PATH.read_text())


@lru_cache(maxsize=None)
def _program(rule: str):
    import celpy

    env = celpy.Environment()
    return env.program(env.compile(rule))


def _rules(validator, rules, instance, schema):
    if not isinstance(instance, dict):
        return
    import celpy
    from celpy import celtypes

    activation = {"self": celpy.json_to_cel(instance)}
    for r in rules:
        try:
            ok = _program(r["rule"]).evaluate(activation) == celtypes.BoolType(True)
        except Exception:
            ok = False
        if not ok:
            yield jsonschema.ValidationError(r["message"], path=[r.get("path")] if r.get("path") else [])


@lru_cache(maxsize=None)
def _validator():
    base = validators.validator_for(schema())
    cls = validators.extend(base, {"x-avenger-rules": _rules})
    return cls(schema())


def _path(error) -> str:
    out = ""
    for p in error.absolute_path:
        out += f"[{p}]" if isinstance(p, int) else (f".{p}" if out else str(p))
    return out or "$"


def validate(spec: dict) -> list:
    """[] if the spec is in Avenger's subset, else (path, message) per error."""
    return [(_path(e), e.message) for e in _validator().iter_errors(spec)]
