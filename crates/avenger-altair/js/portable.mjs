// Validate a spec against Avenger's Vega-Lite subset without Rust, in
// JavaScript: the generated JSON Schema with ajv (draft 2020-12), and the CEL
// rules in `x-avenger-rules` with cel-js, run on every instance node their
// subschema matches (as Kubernetes runs x-kubernetes-validations).
//
//     import {validate} from './portable.mjs'
//     validate(spec)   // [] if Avenger takes it, else [{path, message}, ...]

import {parse} from '@marcbachmann/cel-js'
import Ajv2020 from 'ajv/dist/2020.js'
import {readFileSync} from 'node:fs'

const here = new URL('.', import.meta.url)
export const schema = JSON.parse(readFileSync(new URL('../python/avenger_altair/avenger-vegalite.schema.json', here), 'utf8'))

const programs = new Map()
function program(rule) {
  let p = programs.get(rule)
  if (!p) programs.set(rule, (p = parse(rule)))
  return p
}

const ajv = new Ajv2020({allErrors: true, strict: false, validateFormats: false})
ajv.addKeyword({
  keyword: 'x-avenger-rules',
  errors: true,
  validate: function rules(list, data) {
    if (data === null || typeof data !== 'object' || Array.isArray(data)) return true
    const errors = []
    for (const r of list) {
      let ok
      try {
        ok = program(r.rule)({self: data}) === true
      } catch {
        ok = false
      }
      if (!ok) errors.push({keyword: 'x-avenger-rules', message: r.message, params: {path: r.path}})
    }
    rules.errors = errors
    return errors.length === 0
  },
})
const check = ajv.compile(schema)

export function validate(spec) {
  if (check(spec)) return []
  return check.errors.map(e => ({path: e.instancePath + (e.params?.path ? `/${e.params.path}` : ''), message: e.message}))
}
