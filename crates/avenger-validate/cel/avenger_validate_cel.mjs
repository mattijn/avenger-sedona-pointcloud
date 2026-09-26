// Layers 1 and 2 of avenger-validate, from its CEL export, with cel-js.
//
//     npm install @marcbachmann/cel-js
//     avenger-validate export-cel > bundle.json
//     node avenger_validate_cel.mjs bundle.json steps.json
//
// `steps.json` is a pipeline as data: [{"step": "chart", "args": ["bar"],
// "flags": {"x": "label:N"}}, ...], all values strings. Layers 3 (Vega
// expressions) and 4 (SQL against the schema) need the library.

import {parse} from '@marcbachmann/cel-js'
import {readFileSync} from 'node:fs'

export class Bundle {
  constructor(bundle) {
    this.bundle = bundle
    this.programs = new Map()
  }

  run(src, vars) {
    let p = this.programs.get(src)
    if (!p) this.programs.set(src, (p = parse(src)))
    try {
      return p(vars) === true
    } catch {
      return false
    }
  }

  validate(steps) {
    const issues = []
    const state = {...this.bundle.state}
    steps.forEach((s, i) => {
      const spec = this.bundle.steps[s.step]
      if (!spec) return issues.push({layer: 1, step: i, arg: null, message: `unknown step \`${s.step}\``})
      const args = {}
      const given = s.args ?? []
      spec.positional.forEach((p, k) => {
        if (k < given.length) {
          if (!this.run(p.check, {v: given[k]})) issues.push({layer: 1, step: i, arg: p.name, message: `\`${given[k]}\` is not a ${p.type}`})
          args[p.name] = given[k]
        } else if (!p.optional) issues.push({layer: 1, step: i, arg: p.name, message: `needs its ${p.name}`})
      })
      if (given.length > spec.positional.length) issues.push({layer: 1, step: i, arg: null, message: 'too many positional arguments'})
      for (const [k, v] of Object.entries(s.flags ?? {})) {
        const f = spec.flags[k]
        if (!f) {
          issues.push({layer: 1, step: i, arg: `--${k}`, message: `no flag --${k}`})
          continue
        }
        if (!this.run(f.check, {v})) issues.push({layer: 1, step: i, arg: `--${k}`, message: `\`${v}\` is not a ${f.type}`})
        args[k] = v
      }
      if (!this.run(spec.requires, {state, step: args})) issues.push({layer: 2, step: i, arg: null, message: spec.message ?? `needs ${spec.requires}`})
      for (const [k, v] of Object.entries(spec.sets)) state[k] = v.startsWith('$') ? (args[v.slice(1)] ?? '') : v
    })
    return issues
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const b = new Bundle(JSON.parse(readFileSync(process.argv[2], 'utf8')))
  const issues = b.validate(JSON.parse(readFileSync(process.argv[3], 'utf8')))
  console.log(JSON.stringify(issues, null, 2))
  process.exit(issues.length ? 1 : 0)
}
