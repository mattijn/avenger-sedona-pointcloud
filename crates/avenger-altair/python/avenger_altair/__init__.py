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

__all__ = ["enable", "disable", "enabled", "explain", "live", "LiveChart", "view", "normalise", "render", "save", "to_vegalite", "validate", "AvengerRefusal", "last"]


class AvengerRefusal(ValueError):
    """Avenger does not take this spec: `path` names the property."""

    def __init__(self, refusal: dict):
        self.path = refusal["path"]
        self.stage = refusal["stage"]
        self.reason = refusal["message"]
        what = "refuses this chart" if self.stage == "spec" else "does not draw this chart yet"
        super().__init__(f"Avenger {what}: {self.path}: {self.reason}")


# What the last chart did: "avenger", or "fallback" with the refusal.
last: dict = {}


# Vega-Lite gives a view with a continuous axis the size in
# `config.view.continuousWidth/Height` (Altair's default theme sets both to
# 300). Avenger's spec reads `config` but its compiler does not size by it
# yet, so the size is written out as the explicit width or height Vega-Lite
# would use.
_CONTINUOUS = {"quantitative", "temporal"}


def normalise(spec: dict) -> dict:
    """The spec with `config.view`'s continuous sizes written out."""
    view = (spec.get("config") or {}).get("view") or {}
    enc = spec.get("encoding") or {}
    if not isinstance(enc, dict):
        return spec
    out = None
    for channel, size, key in (("x", "width", "continuousWidth"), ("y", "height", "continuousHeight")):
        ch = enc.get(channel) or {}
        if key in view and size not in spec and isinstance(ch, dict) and ch.get("type") in _CONTINUOUS:
            out = out or dict(spec)
            out[size] = view[key]
    return out or spec


def to_vegalite(spec: dict) -> dict:
    """The spec without what is Avenger's alone (`camera`), for Vega."""
    return {k: v for k, v in spec.items() if k != "camera"}


def _spec(chart_or_spec: Any) -> dict:
    if isinstance(chart_or_spec, dict):
        return normalise(chart_or_spec)
    with alt.data_transformers.disable_max_rows():
        return normalise(chart_or_spec.to_dict(validate=False))


# --- data -------------------------------------------------------------------
#
# Altair's default data transformer writes a DataFrame into the spec as JSON
# rows; for a million values that is most of the time (to_dict, json.dumps,
# and parsing it again in Rust). The "avenger" transformer instead keeps the
# frame and puts only its name in the spec; the renderer hands the frame to
# Avenger as Arrow, through the Arrow PyCapsule interface.

_tables: "dict[str, Any]" = {}
_MAX_TABLES = 64


def _arrow(data: Any) -> Any:
    if hasattr(data, "__arrow_c_stream__"):
        return data
    import pyarrow as pa  # a pandas frame without the interface

    return pa.Table.from_pandas(data, preserve_index=False)


def to_avenger(data: Any) -> dict:
    """Altair data transformer: a frame by name, not as rows."""
    if isinstance(data, dict) or not (hasattr(data, "__arrow_c_stream__") or hasattr(data, "to_numpy")):
        return _default_transformer(data)
    name = f"avenger-{id(data):x}"
    _tables[name] = data
    while len(_tables) > _MAX_TABLES:
        _tables.pop(next(iter(_tables)))
    return {"name": name}


def _default_transformer(data: Any) -> dict:
    return alt.default_data_transformer(data)


def _named(spec: Any, out: set) -> set:
    if isinstance(spec, dict):
        d = spec.get("data")
        if isinstance(d, dict) and isinstance(d.get("name"), str) and d["name"] in _tables:
            out.add(d["name"])
        for v in spec.values():
            _named(v, out)
    elif isinstance(spec, list):
        for v in spec:
            _named(v, out)
    return out


def _bound(spec: dict) -> dict:
    return {n: _arrow(_tables[n]) for n in _named(spec, set())}


def _with_rows(spec: dict) -> dict:
    """The spec with its named frames written in as rows, for a renderer
    that is not Avenger."""
    names = _named(spec, set())
    if not names:
        return spec
    spec = dict(spec)
    datasets = dict(spec.get("datasets") or {})
    for n in names:
        datasets[n] = alt.utils.data.to_values(_tables[n])["values"]
    spec["datasets"] = datasets
    return spec


def validate(chart_or_spec: Any) -> Optional[dict]:
    """None if Avenger takes the spec, else `{"path", "message", "stage"}`."""
    return _native.validate(json.dumps(_spec(chart_or_spec)))


def render(chart_or_spec: Any, format: str = "svg", scale: float = 2.0, base_dir: str | None = None) -> bytes:
    """The chart drawn by Avenger, as SVG (UTF-8) or PNG bytes."""
    spec = _spec(chart_or_spec)
    data, refusal = _native.render(json.dumps(spec), format, scale, base_dir, _bound(spec))
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
        _, r = _native.render(json.dumps(spec), "png", 1.0, None, _bound(spec))
    if r is None:
        return "Avenger draws this chart."
    return f"Avenger does not draw this chart yet ({r['stage']}): {r['path']}: {r['message']}"


# --- live data ------------------------------------------------------------


class LiveChart:
    """A chart compiled once over its DataFrame, redrawn as rows arrive.

        live = av.live(chart)          # chart = alt.Chart(df).mark_bar()...
        live.append(more_rows)         # a DataFrame with the same columns
        live.png()                     # the chart over all rows so far

    The spec is compiled once; each append hands the new rows to Avenger as
    Arrow and the next draw recomputes the dataflow over the whole table.
    """

    def __init__(self, chart: Any):
        spec, frame = _named_spec(chart)
        self._live = _native.Live(json.dumps(spec), _arrow(frame))
        self.spec = spec
        self._views: list = []

    def append(self, frame: Any) -> int:
        """Add rows (a pandas, Polars or PyArrow frame); returns the row count.
        Views of this chart draw again."""
        rows = self._live.append(_arrow(frame))
        for v in self._views:
            v.rows = rows
            v.refresh()
        return rows

    def view(self, scale: float = 2.0, zoom: float = 1.0):
        """An interactive notebook view that follows the rows as they arrive."""
        from .widget import live_view

        return live_view(self, scale, zoom)

    @property
    def rows(self) -> int:
        return self._live.rows

    def png(self, scale: float = 2.0) -> bytes:
        return self._live.render("png", scale)

    def svg(self) -> str:
        return self._live.render("svg", 1.0).decode()

    def _repr_mimebundle_(self, include=None, exclude=None):
        return {"image/png": base64.b64encode(self.png()).decode()}


def _named_spec(chart: Any) -> "tuple[dict, Any]":
    """The chart's spec with its DataFrame as named data, and the frame."""
    alt.data_transformers.register("avenger", to_avenger)
    previous = alt.data_transformers.active
    options = dict(alt.data_transformers.options)
    alt.data_transformers.enable("avenger")
    try:
        spec = normalise(chart.to_dict(validate=False))
    finally:
        alt.data_transformers.enable(previous, **options)
    names = sorted(_named(spec, set()))
    if len(names) != 1:
        raise ValueError("a live chart needs exactly one DataFrame as its data")
    return spec, _tables[names[0]]


def view(chart: Any, scale: float = 2.0, zoom: float = 1.0):
    """An interactive notebook view of the chart, drawn by Avenger in the
    kernel: a fisheye follows the pointer, a tilt turns with a drag."""
    from .widget import view as _view

    return _view(chart, scale, zoom)


def live(chart: Any) -> LiveChart:
    """A chart that grows with its data: `live(chart).append(frame)`."""
    return LiveChart(chart)


# --- validation -------------------------------------------------------------
#
# Altair validates a chart in `to_dict()` by calling `validate` on the
# top-level class with the full spec. Enabled, a spec Avenger takes skips the
# JSON Schema check; one it does not take is checked by the JSON Schema as
# before, so a valid chart outside Avenger's subset stays valid.

_TOP_LEVEL = (alt.Chart,)
_originals: dict = {}


def _avenger_validate(cls, instance, schema=None):
    if schema is None:
        refusal = _native.validate(json.dumps(normalise(instance)))
        if refusal is None:
            return None
        # Grammar only Avenger has cannot be judged by Vega-Lite's schema,
        # which would only say the property does not exist.
        if isinstance(instance, dict) and "camera" in instance:
            raise AvengerRefusal(refusal)
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


# --- camera ---------------------------------------------------------------
#
# `camera` is grammar Avenger has and Vega-Lite has not. Its methods are made
# from the variants in Avenger's schema, as a generated API would be, and put
# on Altair's Chart while Avenger is enabled.

_camera_methods: list = []


def _install_camera() -> None:
    from . import portable

    for variant in portable.schema()["$defs"]["Camera"]["anyOf"]:
        kind = variant["properties"]["type"]["const"]
        args = [p for p in variant["properties"] if p != "type"]

        def method(self, _kind=kind, **kwds):
            unknown = set(kwds) - set(_args[_kind])
            if unknown:
                raise TypeError(f"camera_{_kind}() has no argument {sorted(unknown)[0]!r}")
            return self.properties(camera={"type": _kind, **kwds})

        _args[kind] = args
        method.__name__ = f"camera_{kind}"
        method.__doc__ = (variant.get("description") or f"A {kind} camera.").splitlines()[0] + (f" Arguments: {', '.join(args)}." if args else "")
        for cls in _TOP_LEVEL:
            setattr(cls, method.__name__, method)
        _camera_methods.append(method.__name__)


_args: dict = {}


def _remove_camera() -> None:
    for name in _camera_methods:
        for cls in _TOP_LEVEL:
            if name in cls.__dict__:
                delattr(cls, name)
    _camera_methods.clear()


# --- rendering --------------------------------------------------------------


def _renderer(spec: dict, **kwargs) -> dict:
    fmt = _options.get("format", "png")
    fallback = _options.get("fallback", True)
    data, refusal = _native.render(json.dumps(normalise(spec)), fmt, _options.get("scale", 2.0), None, _bound(spec))
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
    # The renderer that was active before, as `enable` found it, with any
    # frame that went by name written back in as rows.
    return _options["previous_renderer"](to_vegalite(_with_rows(spec)), **kwargs)


_options: dict = {}


def enable(format: str = "png", scale: float = 2.0, fallback: bool = True, validation: bool = True, arrow: bool = True) -> None:
    """Render with Avenger (and validate with it, unless `validation=False`).
    `fallback=False` raises `AvengerRefusal` for a chart Avenger does not draw
    yet, instead of drawing it the usual way. With `arrow` (the default), a
    DataFrame reaches Avenger as Arrow by name instead of as JSON rows."""
    if arrow:
        if alt.data_transformers.active != "avenger":
            _options["previous_data"] = alt.data_transformers.active
            _options["previous_data_options"] = dict(alt.data_transformers.options)
        alt.data_transformers.register("avenger", to_avenger)
        alt.data_transformers.enable("avenger")
    previous = alt.renderers.active
    if previous != "avenger":
        _options["previous"] = previous
        _options["previous_options"] = dict(alt.renderers.options)
        _options["previous_renderer"] = alt.renderers.get()
    _options.update(format=format, scale=scale, fallback=fallback)
    alt.renderers.register("avenger", _renderer)
    alt.renderers.enable("avenger")
    if validation:
        _install_validation()
    _install_camera()


def disable() -> None:
    """Back to the renderer and validation that were active before."""
    _remove_validation()
    _remove_camera()
    if alt.data_transformers.active == "avenger":
        alt.data_transformers.enable(_options.get("previous_data") or "default", **_options.get("previous_data_options", {}))
    if alt.renderers.active == "avenger":
        alt.renderers.enable(_options.get("previous") or "default", **_options.get("previous_options", {}))


@contextmanager
def enabled(**kwargs):
    enable(**kwargs)
    try:
        yield
    finally:
        disable()
