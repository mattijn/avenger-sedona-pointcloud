"""An interactive Avenger view in the notebook (anywidget).

    view = av.view(chart.camera_fisheye())    # the focus follows the pointer
    view = av.view(chart.camera_tilt())       # drag to turn
    live = av.live(chart); view = live.view(); live.append(df)   # new rows appear

The kernel draws every frame with Avenger and sends a PNG; the browser only
shows it and sends the pointer back (widget.js).
"""

from __future__ import annotations

import base64
import json
import time
from pathlib import Path
from typing import Any, Callable, Optional

import anywidget
import traitlets


class AvengerView(anywidget.AnyWidget):
    _esm = Path(__file__).with_name("widget.js")
    png = traitlets.Unicode("").tag(sync=True)
    size = traitlets.List([0.0, 0.0]).tag(sync=True)
    plot = traitlets.List([0.0, 0.0, 0.0, 0.0]).tag(sync=True)
    interaction = traitlets.Unicode("none").tag(sync=True)
    event = traitlets.Dict({}).tag(sync=True)
    seq = traitlets.Int(0).tag(sync=True)
    frame_ms = traitlets.Float(0.0).tag(sync=True)
    status = traitlets.Unicode("").tag(sync=True)

    def __init__(self, draw: Callable[[Optional[dict]], dict], camera: Optional[dict], scale: float = 2.0, **kwargs):
        super().__init__(**kwargs)
        self._draw = draw
        self.scale = scale
        self.camera = dict(camera) if camera else None
        self.interaction = (self.camera or {}).get("type", "none") if (self.camera or {}).get("type") in ("fisheye", "tilt") else "none"
        self.frames = 0
        self.refresh()
        self.observe(self._on_event, names="event")

    def refresh(self) -> None:
        """Draw again: after new rows, or a changed camera."""
        t = time.perf_counter()
        frame = self._draw(self.camera)
        if "refusal" in frame:
            r = frame["refusal"]
            self.status = f"Avenger does not draw this: {r['path']}: {r['message']}"
            return
        self.frame_ms = (time.perf_counter() - t) * 1e3
        self.size, self.plot = frame["size"], frame["plot"]
        self.png = base64.b64encode(frame["png"]).decode()
        self.frames += 1

    def handle(self, event: dict) -> None:
        """What an event does to the camera; then one frame."""
        c = self.camera or {}
        kind = event.get("kind")
        if c.get("type") == "fisheye" and kind in ("move", "drag"):
            px, py, pw, ph = self.plot
            u = min(max((event["x"] - px) / pw, 0.0), 1.0)
            v = min(max(1.0 - (event["y"] - py) / ph, 0.0), 1.0)
            self.camera = {**c, "focus": [round(u, 4), round(v, 4)]}
        elif c.get("type") == "tilt" and kind == "drag":
            yaw = c.get("yaw", 30.0) + 0.5 * event.get("dx", 0.0)
            elevation = min(max(c.get("elevation", 40.0) - 0.3 * event.get("dy", 0.0), 5.0), 89.0)
            self.camera = {**c, "yaw": round(yaw, 2), "elevation": round(elevation, 2)}
        else:
            return
        self.refresh()

    def _on_event(self, change) -> None:
        event = change["new"] or {}
        try:
            self.handle(event)
        finally:
            # Tell the browser this event is done; it sends the next.
            self.seq = event.get("seq", self.seq)


def view(chart: Any, scale: float = 2.0) -> AvengerView:
    """An interactive view of an Altair chart drawn by Avenger."""
    from . import _bound, _native, _spec

    spec = _spec(chart)
    tables = _bound(spec)

    def draw(camera: Optional[dict]) -> dict:
        s = dict(spec, camera=camera) if camera else spec
        return _native.render_frame(json.dumps(s), scale, tables)

    return AvengerView(draw, spec.get("camera"), scale)


def live_view(live: Any, scale: float = 2.0) -> AvengerView:
    def draw(camera: Optional[dict]) -> dict:
        return live._live.render_frame(scale, json.dumps(camera) if camera else None)

    v = AvengerView(draw, live.spec.get("camera"), scale)
    live._views.append(v)
    return v
