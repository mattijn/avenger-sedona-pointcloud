// The Rust validator as wasm, in Node: verdicts and time on experiment 7's
// corpus (layers 0 to 3; no DataFusion in the wasm build).
// node bench_wasm.mjs <pkg dir> <pipelines.jsonl>
import {readFileSync} from 'node:fs'
import {createRequire} from 'node:module'

const require = createRequire(import.meta.url)
let t = performance.now()
const {Validator} = require(`${process.argv[2]}/avenger_validate.js`)
const v = new Validator()
const start = performance.now() - t
const pipes = readFileSync(process.argv[3], 'utf8').trim().split('\n').map(l => JSON.parse(l))
const rs = pipes.map(p => v.check(p.pipeline))
const caught = pipes.filter((p, k) => p.refused && !rs[k].valid).length
const fp = pipes.filter((p, k) => !p.refused && !rs[k].valid).length
console.log(`wasm: start-up ${start.toFixed(1)} ms; refused and flagged ${caught} of ${pipes.filter(p => p.refused).length}; accepted but flagged ${fp}; layers ${rs[0].layers}`)
t = performance.now()
const n = 20
for (let k = 0; k < n; k++) for (const p of pipes) v.check(p.pipeline)
console.log(`wasm: ${(((performance.now() - t) / n / pipes.length) * 1000).toFixed(1)} µs a pipeline (warm)`)
console.log(JSON.stringify(rs.find(r => !r.valid).issues[0]))
