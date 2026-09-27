// The browser side of an Avenger view: show the PNG the kernel drew, send
// the pointer back. No GPU or chart code runs here (jonmmease/avenger#77:
// WebGPU in the frontend is not there everywhere). One event is in flight at
// a time and the newest waiting one replaces older ones, so frames never
// queue up behind a fast pointer.

function render({model, el}) {
  const box = document.createElement('div')
  box.style.cssText = 'display:inline-block;font:12px system-ui,sans-serif;color:#555'
  const img = document.createElement('img')
  img.style.cssText = 'display:block;touch-action:none;user-select:none'
  img.draggable = false
  const status = document.createElement('div')
  status.style.cssText = 'margin-top:4px'
  // A live view's row count: every row so far, whatever the chart aggregates them into.
  const rows = document.createElement('div')
  rows.style.cssText = 'font:600 20px system-ui,sans-serif;color:#222;margin-bottom:6px;font-variant-numeric:tabular-nums'
  box.append(rows, img, status)
  el.append(box)

  const hint = {fisheye: 'move the pointer over the plot', tilt: 'drag to turn', none: ''}
  function draw() {
    const [w, h] = model.get('size')
    const zoom = model.get('zoom') || 1
    img.src = 'data:image/png;base64,' + model.get('png')
    img.style.width = w * zoom + 'px'
    img.style.height = h * zoom + 'px'
    img.style.cursor = model.get('interaction') === 'tilt' ? 'grab' : 'crosshair'
    const n = model.get('rows')
    rows.style.display = n >= 0 ? 'block' : 'none'
    rows.textContent = n >= 0 ? `${n.toLocaleString('en-US')} rows` : ''
    const ms = model.get('frame_ms')
    status.textContent = [model.get('status'), ms ? `frame ${ms.toFixed(0)} ms` : '', hint[model.get('interaction')] || ''].filter(Boolean).join(' · ')
  }
  model.on('change:png', draw)
  model.on('change:status', draw)
  model.on('change:rows', draw)
  draw()

  let pending = null
  let inflight = false
  let seq = 0
  function flush() {
    if (!pending || inflight) return
    inflight = true
    seq += 1
    model.set('event', {...pending, seq})
    model.save_changes()
    pending = null
  }
  // The kernel answers each event by echoing its number.
  model.on('change:seq', () => {
    inflight = false
    flush()
  })
  function send(ev) {
    if (pending && ev.kind === 'drag' && pending.kind === 'drag') {
      pending = {...ev, dx: pending.dx + ev.dx, dy: pending.dy + ev.dy}
    } else {
      pending = ev
    }
    flush()
  }

  function at(e) {
    const r = img.getBoundingClientRect()
    const [w, h] = model.get('size')
    return [((e.clientX - r.left) * w) / r.width, ((e.clientY - r.top) * h) / r.height]
  }
  let last = null
  img.addEventListener('pointerdown', e => {
    last = [e.clientX, e.clientY]
    img.setPointerCapture(e.pointerId)
    if (model.get('interaction') === 'tilt') img.style.cursor = 'grabbing'
  })
  img.addEventListener('pointerup', e => {
    last = null
    img.releasePointerCapture(e.pointerId)
    draw()
  })
  img.addEventListener('pointermove', e => {
    const [x, y] = at(e)
    if (last) {
      const dx = e.clientX - last[0]
      const dy = e.clientY - last[1]
      last = [e.clientX, e.clientY]
      send({kind: 'drag', x, y, dx, dy})
    } else {
      send({kind: 'move', x, y})
    }
  })
  img.addEventListener('pointerleave', () => send({kind: 'leave'}))
}

export default {render}
