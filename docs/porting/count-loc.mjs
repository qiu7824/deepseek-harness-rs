// 统计 D:\HermesTemp\deepseek-harness 各 npm 包源码行数,产出 loc.json
// 归属判定: 每个文件向上找最近的 package.json,取其 "name" 字段;找不到则用目录名。
// 用法: node count-loc.mjs
import { readdirSync, statSync, readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, relative, dirname, basename, sep } from 'node:path'

const ROOT = 'D:/HermesTemp/deepseek-harness'
const SCOPES = ['packages', 'apps', 'vendor', 'python', 'native']
const EXT = new Set(['.ts', '.tsx', '.js', '.mjs', '.cjs', '.py', '.rs', '.c', '.h', '.cpp'])
const SKIP_DIRS = new Set(['node_modules', 'dist', 'lib', '.git', 'coverage', '__pycache__', '.agents'])
const pkgJsonCache = new Map()
const noPkgCache = new Set()

function findPackageName(file) {
  let dir = dirname(file)
  const chain = []
  while (true) {
    chain.push(dir)
    if (noPkgCache.has(dir)) { dir = dirname(dir); continue }
    if (pkgJsonCache.has(dir)) {
      const v = pkgJsonCache.get(dir)
      if (v) { for (const d of chain) pkgJsonCache.set(d, v); return v }
      dir = dirname(dir)
      continue
    }
    const pj = join(dir, 'package.json')
    if (existsSync(pj)) {
      try {
        const name = JSON.parse(readFileSync(pj, 'utf8')).name ?? basename(dir)
        pkgJsonCache.set(dir, name)
        for (const d of chain) pkgJsonCache.set(d, name)
        return name
      } catch {
        pkgJsonCache.set(dir, null)
        noPkgCache.add(dir)
        dir = dirname(dir)
        continue
      }
    }
    noPkgCache.add(dir)
    if (dir === ROOT || dirname(dir) === dir) {
      for (const d of chain) pkgJsonCache.set(d, null)
      return null
    }
    dir = dirname(dir)
  }
}

function walk(dir, cb) {
  let entries
  try { entries = readdirSync(dir, { withFileTypes: true }) } catch { return }
  for (const e of entries) {
    if (SKIP_DIRS.has(e.name)) continue
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, cb)
    else cb(p)
  }
}

const stats = {} // packageName -> {src, tests, config, other, files}
for (const scope of SCOPES) {
  const scopeDir = join(ROOT, scope)
  if (!existsSync(scopeDir)) continue
  walk(scopeDir, (file) => {
    const base = basename(file)
    if (!EXT.has('.' + base.split('.').pop())) return
    const pkg = findPackageName(file)
    if (!pkg) return
    const relPath = relative(scopeDir, file).split(sep).join('/')
    let bucket = 'other'
    if (relPath.includes('/src/')) bucket = 'src'
    else if (relPath.includes('/tests/') || base.includes('.spec.') || base.includes('.test.')) bucket = 'tests'
    else if (relPath.includes('/config/')) bucket = 'config'
    const s = (stats[pkg] ??= { src: 0, tests: 0, config: 0, other: 0, files: 0 })
    try { s[bucket] += readFileSync(file, 'utf8').split('\n').length; s.files++ } catch {}
  })
}

const rows = Object.entries(stats)
  .map(([pkg, v]) => ({ pkg, ...v, total: v.src + v.tests + v.config + v.other }))
  .sort((a, b) => b.total - a.total)
const sum = rows.reduce((a, r) => a + r.total, 0)
writeFileSync(new URL('./loc.json', import.meta.url), JSON.stringify({ generated: new Date().toISOString(), totalLines: sum, packages: rows.length, rows }, null, 2))
console.log(`packages: ${rows.length}, total lines: ${sum}`)
for (const r of rows) console.log(`${String(r.total).padStart(7)}  ${r.pkg}  (src ${r.src}, tests ${r.tests})`)
