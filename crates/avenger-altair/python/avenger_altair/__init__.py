"""Avenger as an opt-in backend for Altair.

    import altair as alt
    import avenger_altair

    avenger_altair.enable()          # render with Avenger, validate with Avenger
    chart = alt.Chart(df).mark_bar().encode(x="category:N", y="sum(amount):Q")
    chart                            # drawn by Avenger, in the notebook
    avenger_altair.save(chart, "bars.svg")

Avenger reads a growing subset of Vega-Lite. A chart outside it is not an
error: it is validated and drawn the usual way, and `explain(chart)` says
which property Avenger does not take yet (`fallback=False` makes that an
error instead).
"""

from __future__ import annotations

import base64
import json
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Optional

import altair as alt

from . import _native

__all__ = ["enable", "disable", "enabled", "explain", "normalise", "render", "save", "validate", "AvengerRefusal", "last"]


class AvengerRefusal(ValueError):
    """Avenger does not take this spec: `path` names the property."""

    def __init__(self, refusal: dict):
        self.path = refusal["path"]
        self.stage = refusal["stage"]
        self.reason = refusal["message"]
        super().__init__(f"Avenger does not draw this chart yet: {self.path}: {self.reason}")


# What the last chart did: "avenger", or "fallback" with the refusal.
last: dict = {}


# Altair's default theme adds this to every chart. Vega-Lite reads it as the
# size of a continuous axis; Avenger takes no `config` yet, so it becomes the
# explicit width or height Vega-Lite would give. Any other config is left in
# place, and Avenger says what it does not take.
_ALTAIR_DEFAULT_CONFIG = {"view": {"continuousWidth": 300, "continuousHeight": 300}}
_CONTINUOUS = {"quantitative", "temporal"}


def normalise(spec: dict) -> dict:
    """The spec with Altair's default theme written out as Vega-Lite applies it."""
    if spec.get("config") != _ALTAIR_DEFAULT_CONFIG:
        return spec
    spec = {k: v for k, v in spec.items() if k != "config"}
    enc = spec.get("encoding") or {}
    for channel, size in (("x", "width"), ("y", "height")):
        ch = enc.get(channel) or {}
        if size not in spec and ch.get("type") in _CONTINUOUS:
            spec[size] = 300
    return spec


def _spec(chart_or_spec: Any) -> dict:
    if isinstance(chart_or_spec, dict):
        return normalise(chart_or_spec)
    with alt.data_transformers.disable_max_rows():
        return normalise(chart_or_spec.to_dict(validate=False))


def validate(chart_or_spec: Any) -> Optional[dict]:
    """None if Avenger takes the spec, else `{"path", "message", "stage"}`."""
    return _native.validate(json.dumps(_spec(chart_or_spec)))


def render(chart_or_spec: Any, format: str = "svg", scale: float = 2.0, base_dir: str | None = None) -> bytes:
    """The chart drawn by Avenger, as SVG (UTF-8) or PNG bytes."""
    data, refusal = _native.render(json.dumps(_spec(chart_or_spec)), format, scale, base_dir)
    if refusal is not None:
        raise AvengerRefusal(refusal)
    return data


def save(chart: Any, path: str | Path, scale: float = 2.0) -> None:
    """Write the chart as .svg or .png, drawn by Avenger."""
    path = Path(path)
    fmt = path.suffix.lstrip(".").lower()
    path.write_bytes(render(chart, fmt, scale))


def explain(chart_or_spec: Any) -> str:
    """Whether Avenger draws this chart, and if not, which property stops it."""
    spec = _spec(chart_or_spec)
    r = _native.validate(json.dumps(spec))
    if r is None:
        _, r = _native.render(json.dumps(spec), "svg", 1.0, None)
    if r is None:
        return "Avenger draws this chart."
    return f"Avenger does not draw this chart yet ({r['stage']}): {r['path']}: {r['message']}"


# --- validation -------------------------------------------------------------
#
# Altair validates a chart in `to_dict()` by calling `validate` on the
# top-level class with the full spec. Enabled, a spec Avenger takes skips the
# JSON Schema check; one it does not take is checked by the JSON Schema as
# before, so a valid chart outside Avenger's subset stays valid.

_TOP_LEVEL = (alt.Chart,)
_originals: dict = {}


def _avenger_validate(cls, instance, schema=None):
    if schema is None and _native.validate(json.dumps(normalise(instance))) is None:
        return None
    return _originals[cls.__name__].__func__(cls, instance, schema)


def _install_validation() -> None:
    for cls in _TOP_LEVEL:
        if cls.__name__ not in _originals:
            _originals[cls.__name__] = cls.__dict__.get("validate") or cls.validate
            cls.validate = classmethod(_avenger_validate)


def _remove_validation() -> None:
    for cls in _TOP_LEVEL:
        orig = _originals.pop(cls.__name__, None)
        if orig is not None:
            if isinstance(orig, classmethod):
                cls.validate = orig
            else:
                del cls.validate


# --- rendering --------------------------------------------------------------


def _renderer(spec: dict, **kwargs) -> dict:
    fmt = _options.get("format", "png")
    fallback = _options.get("fallback", True)
    data, refusal = _native.render(json.dumps(normalise(spec)), fmt, _options.get("scale", 2.0), None)
    if refusal is None:
        last.clear()
        last.update({"backend": "avenger"})
        if fmt == "svg":
            return {"image/svg+xml": data.decode()}
        return {"image/png": base64.b64encode(data).decode()}
    last.clear()
    last.update({"backend": "fallback", **refusal})
    if not fallback:
        raise AvengerRefusal(refusal)
    # The renderer that was active before, as `enable` found it.
    return _options["previous_renderer"](spec, **kwargs)


_options: dict = {}


def enable(format: str = "png", scale: float = 2.0, fallback: bool = True, validation: bool = True) -> None:
    """Render with Avenger (and validate with it, unless `validation=False`).
    `fallback=False` raises `AvengerRefusal` for a chart Avenger does not draw
    yet, instead of drawing it the usual way."""
    previous = alt.renderers.active
    if previous != "avenger":
        _options["previous"] = previous
        _options["previous_renderer"] = alt.renderers.get()
    _options.update(format=format, scale=scale, fallback=fallback)
    alt.renderers.register("avenger", _renderer)
    alt.renderers.enable("avenger")
    if validation:
        _install_validation()


def disable() -> None:
    """Back to the renderer and validation that were active before."""
    _remove_validation()
    if alt.renderers.active == "avenger":
        alt.renderers.enable(_options.get("previous") or "default")


@contextmanager
def enabled(**kwargs):
    enable(**kwargs)
    try:
        yield
    finally:
        disable()
