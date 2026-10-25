#!/usr/bin/env node
/** Package the assembled runtime tree as an offline repair archive.
 *
 *  Output (next to the installer, not inside it):
 *    runtime-<version>-<target>.tar.gz
 *    runtime-<version>-<target>.tar.gz.sha256
 *
 *  The archive is the second half of the runtime contract implemented in
 *  `src-tauri/src/runtime_pack.rs`: `<userData>/runtime/versions/<digest>` is
 *  filled from it only after every byte has been checked against `manifest.json`.
 *  A `.tar.gz` is deliberate — the publishing host already has `tar`, and the
 *  carrier decodes gzip with a pure-Rust dependency, so neither side needs
 *  7-Zip (which the Electron carrier required through `7zip-bin`).
 *
 *  Usage: node scripts/make-runtime-pack.mjs [--runtime <dir>] [--out <dir>] [--target <label>]
 */
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const args = process.argv.slice(2)
const option = (name, fallback) => {
  const index = args.indexOf(name)
  return index >= 0 ? args[index + 1] : fallback
}

const defaultTarget = process.platform === 'win32' ? 'win-x64' : `mac-${process.arch}`
const target = option('--target', defaultTarget)
const runtime = resolve(option('--runtime', join(root, 'build', 'workdsh-runtime')))
const outDir = resolve(option('--out', join(root, 'build', 'packages')))
const version = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8')).version

if (!existsSync(join(runtime, 'primary-runtime', 'runtime.json'))) {
  throw new Error(`Not an assembled WorkDSH runtime (missing primary-runtime/runtime.json): ${runtime}`)
}

/** Keep in sync with `RuntimeLayout::required_files()` in src-tauri/src/runtime_pack.rs. */
const required = [
  'primary-runtime/runtime.json',
  'profiles/workdsh/package.json',
  'profiles/workdsh/cordis.patch.yml',
  'profiles/workdsh/profile-installation.json',
  'profiles/workdsh/node_modules/@deepseek-ai/dsh/lib/bin.js',
  'profiles/workdsh/node_modules/@deepseek-ai/dsh/lib/profile-boot.js',
  'profiles/workdsh/node_modules/@deepseek-ai/dsh-app-boot/lib/index.js',
  `primary-runtime/dependencies/node/bin/${process.platform === 'win32' ? 'node.exe' : 'node'}`,
  'primary-runtime/dependencies/pnpm/bin/pnpm.cjs',
  process.platform === 'win32'
    ? 'primary-runtime/dependencies/python/python.exe'
    : 'primary-runtime/dependencies/python/bin/python3',
]
const missing = required.filter(path => !existsSync(join(runtime, path)))
if (missing.length) {
  // Packaging an incomplete tree would produce an archive the carrier rejects at
  // import time; failing here keeps the mistake at the publisher, not the user.
  throw new Error(`Runtime is incomplete, refusing to package:\n  ${missing.join('\n  ')}`)
}

const metadata = JSON.parse(readFileSync(join(runtime, 'primary-runtime', 'runtime.json'), 'utf8'))
const manifestPath = join(runtime, 'manifest.json')

function walk(directory, files = []) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) walk(path, files)
    else if (entry.isFile()) files.push(path)
  }
  return files
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

const entries = walk(runtime)
  .filter(path => path !== manifestPath)
  .map(path => ({
    path: relative(runtime, path).split(sep).join('/'),
    size: statSync(path).size,
    sha256: sha256(path),
  }))
  .sort((first, second) => first.path.localeCompare(second.path))

const manifest = {
  protocol: 1,
  product: 'WorkDSH',
  version,
  target,
  // Node vocabulary: the carrier compares these with `process.platform`/`process.arch`.
  runtime: {
    platform: process.platform,
    arch: process.arch,
    node: metadata.node ?? '',
    pnpm: metadata.pnpm ?? '',
    python: metadata.python ?? '',
  },
  entries,
}

const archive = join(outDir, `runtime-${version}-${target}.tar.gz`)
mkdirSync(outDir, { recursive: true })
writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + '\n')

try {
  const packed = spawnSync('tar', ['-czf', archive, '-C', runtime, '.'], { stdio: 'inherit' })
  if (packed.error) throw new Error(`Failed to run tar: ${packed.error.message}`)
  if (packed.status !== 0) throw new Error(`tar exited with ${packed.status}`)
} finally {
  // The manifest only exists inside the archive: leaving it in the shared build
  // output would put a Tauri-only file into the bundled resource tree.
  rmSync(manifestPath, { force: true })
}

const digest = sha256(archive)
writeFileSync(archive + '.sha256', `${digest}  runtime-${version}-${target}.tar.gz\n`)
console.log(
  `Runtime pack: ${archive} (${(statSync(archive).size / 1024 / 1024).toFixed(1)} MiB, ${entries.length} files)\n` +
    `sha256: ${digest}`,
)
