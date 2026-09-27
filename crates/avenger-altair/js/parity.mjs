// Judge the specs of bench/schema_parity.py (written with --dump) again in
// JavaScript and compare with Rust's verdicts.
//     node parity.mjs <specs.jsonl>
import {readFileSync} from 'node:fs'
import {validate} from './portable.mjs'

const rows = readFileSync(process.argv[2], 'utf8').trim().split('\n').map(l => JSON.parse(l))
let agree = 0
const differs = []
const t0 = performance.now()
for (const r of rows) {
  const ok = validate(r.spec).length === 0
  if (ok === r.rust_valid) agree++
  else differs.push(r)
}
const first = (performance.now() - t0) / rows.length
const t1 = performance.now()
for (let k = 0; k < 5; k++) for (const r of rows) validate(r.spec)
const warm = (performance.now() - t1) / rows.length / 5
console.log(`${rows.length} specs: ${agree} agree with Rust (${((100 * agree) / rows.length).toFixed(2)} %); ${(first * 1000).toFixed(0)} µs a spec on the first pass, ${(warm * 1000).toFixed(1)} µs warm`)
for (const r of differs.slice(0, 20)) console.log(`  DIFFERS ${r.seed} | ${r.label} | Rust ${r.rust_valid ? 'valid' : 'refuses'} | js ${JSON.stringify(validate(r.spec).slice(0, 2))}`)
