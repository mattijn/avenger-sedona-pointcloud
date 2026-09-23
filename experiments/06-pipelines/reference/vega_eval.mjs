// Evaluate each expression with real Vega: a one-dataset spec with a
// formula transform per expression. Run with TZ=UTC so date functions match.
//   TZ=UTC node vega_eval.mjs ../corpus/differential.json > out.json
import * as vega from 'vega';
import { readFileSync } from 'node:fs';

const cases = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const special = { NaN: NaN, Infinity: Infinity, '-Infinity': -Infinity };
const decode = (v) => (v && typeof v === 'object' && '$' in v ? special[v.$] : v);
const rows = () => cases.rows.map((r) => Object.fromEntries(Object.entries(r).map(([k, v]) => [k, decode(v)])));
const encode = (v) => {
  if (v === undefined) return { $: 'undefined' };
  if (typeof v === 'number' && !Number.isFinite(v)) return { $: String(v) };
  if (v instanceof Date) return { $: 'date', ms: v.getTime() };
  if (Array.isArray(v)) return { $: 'array', value: v.map(encode) };
  return v;
};

const out = {};
for (const expr of cases.expressions) {
  const spec = { data: [{ name: 't', values: rows(), transform: [{ type: 'formula', expr, as: 'out' }] }] };
  try {
    const view = new vega.View(vega.parse(spec), { renderer: 'none' });
    await view.runAsync();
    out[expr] = { values: view.data('t').map((r) => encode(r.out)) };
    view.finalize();
  } catch (e) {
    out[expr] = { error: String(e.message || e) };
  }
}
console.log(JSON.stringify({ vega: vega.version, results: out }, null, 1));
