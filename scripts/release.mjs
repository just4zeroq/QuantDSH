#!/usr/bin/env node
/** Release orchestration for the QuantDSH Tauri carrier.
 *  Mirrors the quantdsh Electron release chain:
 *    prepare:deployment-config → pack profile → prepare runtime → prepare primary runtime
 *    → prepare command → generate icons → make runtime archive
 *    → tauri build → verify packaged artifact
 *  Usage:
 *    node scripts/release.mjs [--target win-x64|mac-arm64|mac-x64] [--source <quantdsh root>] [--skip-build] [-- <extra tauri args>]
 */
import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync, renameSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const args = process.argv.slice(2)
const tauriArgsIndex = args.indexOf('--')
const passthrough = tauriArgsIndex >= 0 ? args.slice(tauriArgsIndex + 1) : []
const ownArgs = tauriArgsIndex >= 0 ? args.slice(0, tauriArgsIndex) : args

function option(name, fallback) {
  const index = ownArgs.indexOf(name)
  return index >= 0 ? ownArgs[index + 1] : fallback
}

// `pack-quantdsh-profile.mjs` reads `<source>/package.json` for two things: the
// `@deepseek-ai/dsh` devDependency pin and `node_modules/pnpm`. Only the Web
// application (`apps/web`) satisfies both; the repository root declares neither.
// Its `PACKAGE_DIRECTORIES` resolve `../../packages/...`, which is the repo's
// `packages/` when the source is `apps/web` — exactly what `apps/desktop`
// README instructs (`node apps/desktop/scripts/pack-quantdsh-profile.mjs apps/web`).
const source = resolve(option('--source', process.env.QUANTDSH_SOURCE ?? join(root, '..', 'quantdsh', 'apps', 'web')))
const skipBuild = ownArgs.includes('--skip-build')
const platform = process.platform
const defaultTarget = platform === 'win32' ? 'win-x64' : `mac-${process.env.QUANTDSH_MAC_ARCH ?? process.arch}`
const target = option('--target', defaultTarget)
if (!['win-x64', 'mac-arm64', 'mac-x64'].includes(target)) throw new Error(`Unsupported release target: ${target}`)
if (target.startsWith('win-') && platform !== 'win32') throw new Error('Windows artifacts must be built on a native Windows host')
if (target.startsWith('mac-') && platform !== 'darwin') throw new Error('macOS artifacts must be built on a native macOS host')

if (!existsSync(join(source, 'package.json'))) throw new Error(`Built QuantDSH Web source not found: ${source}`)
if (!existsSync(join(source, 'node_modules', 'pnpm', 'bin', 'pnpm.cjs'))) {
  throw new Error(`QuantDSH Web dependencies are missing: ${source} (run \`corepack pnpm --dir apps/web install --frozen-lockfile\` and \`corepack pnpm --dir apps/web build\` in the quantdsh checkout first)`)
}

function run(command, commandArgs, env = process.env) {
  console.log(`\n==> ${command} ${commandArgs.join(' ')}`)
  const result = spawnSync(command, commandArgs, { cwd: root, stdio: 'inherit', env, shell: process.platform === 'win32' })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`)
}

const node = process.execPath
const script = name => join(root, 'scripts', name)

// 1. Administrator-selected deployment configuration for this package.
run(node, [script('prepare-deployment-config.mjs')])
// 2. Pack the built owned sources into immutable archives.
// quantdsh's own release entry (`scripts/package-desktop-release.mjs`) always
// packs into a timestamped candidate directory, because `pack-quantdsh-profile.mjs`
// refuses to overwrite one ("candidate archives are immutable"). A fixed path
// here would make the second release of the same checkout fail, so mirror the
// timestamped naming and hand the candidate to runtime preparation explicitly.
const releaseDirectory = join(root, 'build', `quantdsh-profile-release-${Date.now()}`)
run(node, [script('pack-quantdsh-profile.mjs'), source, releaseDirectory])
// 3. Compose and verify the portable runtime Profile.
run(node, [script('prepare-quantdsh-runtime.mjs')], { ...process.env, QUANTDSH_RELEASE_DIRECTORY: releaseDirectory })
// 4. Attach the locked official primary runtime (Node/pnpm/Python + office skills).
run(node, [script('prepare-quantdsh-primary-runtime.mjs')], { ...process.env, QUANTDSH_MAC_ARCH: target === 'mac-x64' ? 'x64' : target === 'mac-arm64' ? 'arm64' : undefined })
// 5. Ship the official command manager and the installed dsh command.
run(node, [script('prepare-quantdsh-command.mjs')], { ...process.env, QUANTDSH_MAC_ARCH: target === 'mac-x64' ? 'x64' : target === 'mac-arm64' ? 'arm64' : undefined })
// 6. Derive the Tauri icon set from the shared brand artwork.
run(node, [script('generate-icons.mjs')])
// 7. Package the same runtime tree as the offline repair archive. A damaged or
//    missing runtime is repaired by importing this file, without re-downloading
//    the installer; `src-tauri/src/runtime_pack.rs` owns the receiving side.
run(node, [script('make-runtime-pack.mjs'), '--target', target])

if (!skipBuild) {
  // 8. Build and bundle the Tauri carrier.
  const tauri = process.platform === 'win32' ? 'npx.cmd' : 'npx'
  const buildArgs = ['tauri', 'build']
  if (target === 'mac-x64') buildArgs.push('--target', 'x86_64-apple-darwin')
  if (target === 'mac-arm64') buildArgs.push('--target', 'aarch64-apple-darwin')
  // electron-builder shipped exactly one artifact per platform: an x64 NSIS
  // Setup.exe on Windows (`win.target = [{ nsis, x64 }]`) and an unsigned .app
  // on macOS (`mac.target = ['dir']`). Tauri's `all` target would add an MSI and
  // a DMG, so the bundle list is pinned per platform instead.
  buildArgs.push('--bundles', target.startsWith('win-') ? 'nsis' : 'app')
  run(tauri, [...buildArgs, ...passthrough])

  if (target.startsWith('mac-')) {
    // Tauri cannot express electron-builder's `mac.extendInfo`, so the bundled
    // Info.plist is patched right after bundling (LSUIElement keeps a spawned
    // browser-worker child from flashing a Dock icon). Changing the bundle
    // invalidates the signature Tauri produced, so re-sign the outer bundle when
    // an identity is configured; inner binaries keep their own signatures.
    const triple = target === 'mac-x64' ? 'x86_64-apple-darwin' : 'aarch64-apple-darwin'
    const app = join(root, 'src-tauri', 'target', triple, 'release', 'bundle', 'macos', 'QuantDSH.app')
    if (!existsSync(app)) throw new Error(`macOS bundle not found: ${app}`)
    run(node, [script('patch-macos-info-plist.mjs'), '--app', app])
    const identity = process.env.APPLE_SIGNING_IDENTITY
    if (identity) {
      run('codesign', ['--force', '--sign', identity, '--options', 'runtime',
        '--entitlements', join(root, 'src-tauri', 'entitlements.mac.plist'), app])
    } else {
      console.log('No APPLE_SIGNING_IDENTITY set: the patched bundle stays unsigned, as the Electron "dir" target was when unsigned.')
    }
  }
}

// 9. Verify the produced artifact contains exactly the pinned runtime.
const bundleRoot = join(root, 'src-tauri', 'target', 'release', 'bundle')
const candidates = []
function collect(directory, depth = 0) {
  if (!existsSync(directory) || depth > 3) return
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) collect(path, depth + 1)
    else if (/\.(exe|dmg)$/i.test(entry.name)) candidates.push(path)
  }
}
collect(bundleRoot)
if (!candidates.length) throw new Error(`No installer artifact found under ${bundleRoot}`)
candidates.sort((first, second) => statSync(second).mtimeMs - statSync(first).mtimeMs)
// Keep the Electron-era artifact name shape (`QuantDSH-<version>-<arch>-Setup.exe`)
// so release notes, download links and downstream scripts that key off the filename
// keep working; Tauri names NSIS output `<product>_<version>_<arch>-setup.exe`.
if (target === 'win-x64' && /-setup\.exe$/i.test(candidates[0])) {
  const version = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8')).version
  const renamed = join(dirname(candidates[0]), `QuantDSH-${version}-x64-Setup.exe`)
  if (renamed !== candidates[0]) {
    renameSync(candidates[0], renamed)
    candidates[0] = renamed
  }
}
run(node, [script('verify-packaged.mjs'), '--installer', candidates[0]])
console.log(`\nRelease verified: ${candidates[0]}`)
