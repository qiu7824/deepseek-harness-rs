import { readFile, mkdir, writeFile } from 'node:fs/promises'

const id = 'dsh-task-manager'
const source = await readFile(new URL('./src/client/index.js', import.meta.url), 'utf8')
const body = source
  .replace("import { createElement, useEffect, useId, useMemo, useRef, useState } from 'react'", "const React = require('react')\nconst { createElement, useEffect, useId, useMemo, useRef, useState } = React")
  .replace('export const inject =', 'const inject =')
  .replace('export function apply(ctx)', 'function apply(ctx)')
const output = `window.__ModuleLoader__.load({\n  id: ${JSON.stringify(id)},\n  factory: (require) => {\n    const module = { exports: {} }\n    const exports = module.exports\n${body.split('\n').map((line) => line === '' ? '' : `    ${line}`).join('\n')}\n    exports.apply = apply\n    exports.inject = inject\n    return module.exports\n  }\n})\n`
await mkdir(new URL('./lib/', import.meta.url), { recursive: true })
await writeFile(new URL('./lib/client.js', import.meta.url), output)
