import {readFileSync} from 'node:fs'
import {Bundle} from './avenger_validate_cel.mjs'

const b = new Bundle(JSON.parse(readFileSync(process.argv[2], 'utf8')))
const rows = readFileSync(process.argv[3], 'utf8').trim().split('\n').map(l => JSON.parse(l))
const key = is => JSON.stringify(is.map(i => [i.layer, i.step]))
const t = performance.now()
const got = rows.map(r => key(b.validate(r.steps)))
const us = ((performance.now() - t) / rows.length) * 1000
const t2 = performance.now()
for (let k = 0; k < 20; k++) for (const r of rows) b.validate(r.steps)
const warm = ((performance.now() - t2) / rows.length / 20) * 1000
const bad = rows.filter((r, k) => key(r.issues) !== got[k])
console.log(`cel-js: ${rows.length - bad.length} of ${rows.length} agree with Rust (${us.toFixed(0)} µs a pipeline first pass, ${warm.toFixed(1)} µs warm)`)
for (const r of bad.slice(0, 5)) console.log('  differs:', JSON.stringify(r.steps), key(r.issues))
