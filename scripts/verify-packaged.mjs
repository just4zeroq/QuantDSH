#!/usr/bin/env node
/** Verify that the Tauri carrier contains the pinned official runtime and nothing else.
 *  Faithful port of the upstream desktop's verify-carrier script (afterPack),
 *  adapted to the Tauri bundle layout. Usage:
 *    node scripts/verify-packaged.mjs --root <dir containing quantdsh-runtime>
 *    node scripts/verify-packaged.mjs --installer <path to Setup.exe / dmg>   (extracts first)
 */
import { spawnSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve, basename } from 'node:path'
import { fileURLToPath } from 'node:url'
import sharp from 'sharp'
import { DSH_VERSION, OFFICIAL_PNPM_VERSION } from './runtime-version.mjs'
import { ENTERPRISE_PACKAGES, PRODUCT_PACKAGES, RELEASE_PACKAGES } from './quantdsh-package-boundary.mjs'
import { verifyBuiltPackageExports, verifyDefaultComposition, verifyDefaultProfile, verifyInstalledDshVersions, verifyOfficialWebPackages, verifyProfileRelease, verifyReleaseArchives } from './verify-profile-release.mjs'
import { parseDeploymentConfig } from './prepare-deployment-config.mjs'
import { verifyNativeWorkbenchFrame } from './native-workbench-frame-patch.mjs'
import { verifyOfficialRuntimeProvenance } from './official-runtime-provenance.mjs'

const root = resolve(fileURLToPath(new URL('..', import.meta.url)))
const require = createRequire(import.meta.url)
// `7zip-bin` renamed its standalone binary across majors: 4.x exported `path7z`
// (which was really `7za`), 5.x exports `path7za`. Reading only `path7z` left
// `spawnSync` with `undefined` on a 5.x install. Take whichever this install
// actually ships, and let `--installer` degrade on its own when the binary that
// exists cannot read an NSIS container (see `extractInstaller`).
const sevenZipModule = require('7zip-bin')
const sevenZip = process.env.QUANTDSH_7ZIP
  ?? [sevenZipModule.path7z, sevenZipModule.path7za, sevenZipModule.path7x].find(path => path && existsSync(path))

function run(command, args, options = {}) {
  // `inherit` streams progress for the long archive and bundling steps, but it
  // also throws the child's stdout away. A caller that passes `encoding` wants
  // to assert on that output (the packaged pnpm version, the plugin inventory),
  // so keep the pipe for it — otherwise `result.stdout` is null and every
  // string comparison against it fails with a misleading message.
  const stdio = options.encoding === undefined ? 'inherit' : 'pipe'
  const result = spawnSync(command, args, { stdio, timeout: 300_000, ...options })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`)
  return result
}

function findPackagedRoot(directory, depth = 0) {
  if (existsSync(join(directory, 'quantdsh-runtime', 'primary-runtime', 'runtime.json'))) return directory
  if (existsSync(join(directory, 'Resources', 'quantdsh-runtime', 'primary-runtime', 'runtime.json'))) return join(directory, 'Resources')
  if (depth > 4) return undefined
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    const found = findPackagedRoot(join(directory, entry.name), depth + 1)
    if (found) return found
  }
}

function extractInstaller(installer) {
  const target = mkdtempSync(join(tmpdir(), 'quantdsh-verify-'))
  if (/\.dmg$/i.test(installer)) {
    const mount = mkdtempSync(join(tmpdir(), 'quantdsh-mount-'))
    try {
      run('hdiutil', ['attach', '-nobrowse', '-mountpoint', mount, installer])
      const app = readdirSync(mount).find(name => /\.app$/i.test(name))
      if (!app) throw new Error('DMG contains no application bundle')
      run('cp', ['-R', join(mount, app), join(target, app)])
      return { directory: target, cleanup: () => { try { run('hdiutil', ['detach', mount]) } finally { rmSync(target, { recursive: true, force: true }) } } }
    } catch (error) {
      try { run('hdiutil', ['detach', mount]) } catch { /* already detached */ }
      rmSync(target, { recursive: true, force: true })
      throw error
    }
  }
  if (!sevenZip) return installSilently(installer, target)
  // `7za` rejects an NSIS container immediately ("Cannot open the file as
  // archive") rather than half-extracting it, so a non-zero status here means
  // the format is unsupported, not that the payload is damaged. Capture the
  // tool's output instead of inheriting it: this probe is expected to fail on a
  // stock `7zip-bin`, and dumping its error to stderr makes a successful run
  // look like a failure to every caller that checks the stream.
  const extracted = spawnSync(sevenZip, ['x', '-y', installer, `-o${target}`], { encoding: 'utf8', timeout: 1_800_000 })
  if (extracted.error) throw extracted.error
  if (extracted.status === 0) return { directory: target, cleanup: () => rmSync(target, { recursive: true, force: true }) }
  console.log(`${basename(sevenZip)} cannot open ${basename(installer)} as an archive; extracting it by installing instead.`)
  rmSync(target, { recursive: true, force: true })
  return installSilently(installer, target)
}

/** Windows fallback for `--installer` when no archive tool can open the payload.
 *
 *  Tauri's NSIS installer is an lzma-solid NSIS container. `7zip-bin` ships the
 *  standalone `7za`, whose build has no NSIS handler, so it refuses the file
 *  outright; electron-builder's `extraResources` path never needed one because
 *  it ships a directory, not an installer. Installing silently into the
 *  temporary directory is strictly stronger evidence than extraction: it runs
 *  the same script a user runs, including the preinstall hooks, before the
 *  contents are inspected. `/D` must stay the last argument and must not be
 *  quoted — that is an NSIS rule, not a shell one.
 */
function installSilently(installer, directory) {
  if (process.platform !== 'win32') {
    rmSync(directory, { recursive: true, force: true })
    throw new Error(`Cannot extract ${basename(installer)}: ${sevenZip ?? 'no 7-Zip'} has no NSIS handler`)
  }
  console.log(`No NSIS-capable 7-Zip (${sevenZip ?? 'none'}): installing ${basename(installer)} into ${directory} instead.`)
  const installed = spawnSync(installer, ['/S', `/D=${directory}`], { stdio: 'inherit', timeout: 1_800_000 })
  if (installed.error) throw installed.error
  if (installed.status !== 0) throw new Error(`${basename(installer)} exited with ${installed.status}`)

  const uninstaller = join(directory, 'uninstall.exe')
  return {
    directory,
    cleanup: () => {
      if (existsSync(uninstaller)) {
        // `_?=` keeps the uninstaller from copying itself to %TEMP% and
        // returning immediately, so the status below is the real outcome: the
        // payload, the registry entry and the shortcuts are gone once it exits.
        // The one exception is `uninstall.exe` itself — NSIS cannot delete a
        // running image, and its usual workaround (relaunch from %TEMP%) is
        // exactly what `_?=` suppresses, so the following rmSync removes it.
        const removed = spawnSync(uninstaller, ['/S', `_?=${directory}`], { stdio: 'inherit', timeout: 1_800_000 })
        if (removed.status !== 0) console.warn(`Silent uninstall exited with ${removed.status}; remove ${directory} by hand.`)
      }
      try { rmSync(directory, { recursive: true, force: true }) }
      catch { console.warn(`Left the temporary installation behind: ${directory}`) }
    },
  }
}

async function verifyPackagedCarrier(packagedRoot) {
  const platform = process.platform === 'win32' ? 'win32' : process.platform === 'darwin' ? 'darwin' : undefined
  const target = platform === 'win32' ? 'win-x64' : `mac-${process.env.QUANTDSH_MAC_ARCH ?? process.arch}`
  if (!platform) throw new Error('Unsupported packaged binary resource target')

  // Deployment configuration must match this build byte for byte.
  const packagedDeployment = readFileSync(join(packagedRoot, 'quantdsh-config.json'))
  const generatedDeployment = readFileSync(join(root, 'build/deployment/quantdsh-config.json'))
  parseDeploymentConfig(JSON.parse(packagedDeployment.toString('utf8')))
  if (!packagedDeployment.equals(generatedDeployment)) throw new Error('Packaged deployment configuration differs from this build')

  const runtime = join(packagedRoot, 'quantdsh-runtime')
  if (existsSync(join(runtime, 'node'))) throw new Error('Desktop contains a duplicate Node runtime outside the official primary runtime')
  const primary = JSON.parse(readFileSync(join(runtime, 'primary-runtime', 'runtime.json'), 'utf8'))
  if (!primary.pnpm || primary.pnpm !== OFFICIAL_PNPM_VERSION) throw new Error('Packaged official primary runtime is missing or has another pnpm version')
  verifyOfficialRuntimeProvenance(join(runtime, 'primary-runtime'), target)

  const profile = join(runtime, 'profiles', 'quantdsh')
  // Inspect copied bytes: a preparation log cannot prove the frame reached this installer.
  // This one verifier reads asynchronously (it compares hashes), so it must be
  // awaited: without it the caller's `finally` deletes an extracted or silently
  // installed tree while the reads are still queued, and the run fails with a
  // misleading ENOENT after every other assertion already passed.
  await verifyNativeWorkbenchFrame(profile)
  const cache = join(runtime, 'package-cache')
  const release = JSON.parse(readFileSync(join(cache, 'release-manifest.json'), 'utf8'))
  verifyProfileRelease(release, DSH_VERSION, RELEASE_PACKAGES)
  verifyPackagedOwnedExports(profile, release.packages.map(item => item.name))
  verifyReleaseArchives(release, cache)
  verifyOfficialWebPackages(release, profile)
  const profileManifest = JSON.parse(readFileSync(join(profile, 'package.json'), 'utf8'))
  verifyDefaultProfile(profileManifest)
  const installation = JSON.parse(readFileSync(join(profile, 'profile-installation.json'), 'utf8'))
  if (JSON.stringify(installation.dependencies) !== JSON.stringify(profileManifest.dependencies) || installation.optionalDependencies !== undefined) {
    throw new Error('Installation anchor is not derived from the one default Profile manifest')
  }
  for (const name of Object.keys(profileManifest.optionalDependencies ?? {})) {
    const version = JSON.parse(readFileSync(join(profile, 'node_modules', name, 'package.json'), 'utf8')).version
    if (installation.peerDependencies?.[name] !== version) throw new Error('Installation infrastructure/enterprise peer differs from installed archive: ' + name)
  }
  const productPackages = new Set(PRODUCT_PACKAGES)
  const selected = profileManifest.dsh?.profile?.bundles ?? []
  const selectedWorkdsh = selected.filter(name => name.startsWith('quantdsh-'))
  const directWorkdsh = Object.keys(profileManifest.dependencies ?? {}).filter(name => name.startsWith('quantdsh-'))
  if (selectedWorkdsh.length !== productPackages.size || selectedWorkdsh.some(name => !productPackages.has(name))) {
    throw new Error(`Desktop must select exactly ${productPackages.size} QuantDSH product bundles, found ${selectedWorkdsh.join(', ')}`)
  }
  if (directWorkdsh.length !== productPackages.size || directWorkdsh.some(name => !productPackages.has(name))) {
    throw new Error(`Desktop must directly install exactly ${productPackages.size} QuantDSH product bundles, found ${directWorkdsh.join(', ')}`)
  }
  const lockfile = readFileSync(join(profile, 'pnpm-lock.yaml'), 'utf8')
  for (const item of release.packages) {
    if (!existsSync(join(cache, item.filename))) throw new Error(`Bundled plugin archive is missing: ${item.filename}`)
    const installed = JSON.parse(readFileSync(join(profile, 'node_modules', item.name, 'package.json'), 'utf8'))
    if (installed.version !== item.version) throw new Error(`Bundled owned version mismatch: ${item.name}`)
    if (!productPackages.has(item.name) && Object.hasOwn(profileManifest.dependencies ?? {}, item.name)) {
      throw new Error(`Internal QuantDSH service is a direct product dependency: ${item.name}`)
    }
    const dependencies = productPackages.has(item.name) ? profileManifest.dependencies : profileManifest.optionalDependencies
    if (dependencies?.[item.name]?.replaceAll('\\', '/') !== `file:../../package-cache/${item.filename}`) {
      throw new Error(`Bundled ${item.name} must use a portable archive path in its expected dependency section`)
    }
    if (ENTERPRISE_PACKAGES.includes(item.name) && (!installed.exports?.['./desktop'] || installed.peerDependencies?.['@deepseek-ai/dsh'] !== DSH_VERSION)) {
      throw new Error(`Shipped ${item.name} has no compatible Desktop enterprise entry`)
    }
  }
  const patch = readFileSync(join(profile, 'cordis.patch.yml'), 'utf8')
  verifyDefaultComposition(patch)
  for (const name of release.packages.map(item => item.name).filter(name => !productPackages.has(name) && !ENTERPRISE_PACKAGES.includes(name))) {
    if (['workdsh-contracts', 'workdsh-ui', 'workdsh-provider-browser-session'].includes(name)) continue // inserted by the bundle patch
    if (!patch.includes(`name: ${name}`)) throw new Error(`Internal QuantDSH service is missing from Profile patch: ${name}`)
  }
  if (/file:(?:\/|[a-z]:)/iu.test(lockfile)) throw new Error('Bundled plugin lockfile contains a build-machine path')

  const packages = join(profile, 'node_modules', '@deepseek-ai')
  const names = readdirSync(packages).filter(name => name === 'dsh' || name.startsWith('dsh-'))
  const instances = verifyInstalledDshVersions(profile, DSH_VERSION)
  const node = join(runtime, 'primary-runtime', 'dependencies', 'node', 'bin', platform === 'win32' ? 'node.exe' : 'node')
  if (!existsSync(node)) throw new Error('Packaged Node runtime is missing: ' + node)

  // The packaged Node must load the official native plugin dependency.
  const native = run(node, ['--input-type=module', '-e',
    "import { createRequire } from 'node:module'; const root = createRequire(process.argv[1]); createRequire(root.resolve('@deepseek-ai/cordis-plugin-loader'))('node-addon-require-builtin'); console.log('native runtime load passed')", join(profile, 'package.json')],
    { encoding: 'utf8', timeout: 30_000 })
  const pnpm = join(runtime, 'primary-runtime', 'dependencies', 'pnpm', 'bin', 'pnpm.cjs')
  if (!existsSync(pnpm)) throw new Error('Packaged official pnpm CLI is missing')
  const packageManager = run(node, [pnpm, '--version'], { encoding: 'utf8', timeout: 30_000 })
  if (String(packageManager.stdout).trim() !== primary.pnpm) throw new Error('Packaged official pnpm JavaScript CLI cannot run with its bundled Node')

  const inventory = run(node, [join(root, 'scripts/verify-product-plugin-inventory.mjs'), runtime], { encoding: 'utf8', timeout: 120_000 })
  const host = run(node, [join(root, 'scripts/verify-installation-anchor.mjs'), runtime], { encoding: 'utf8', timeout: 150_000, maxBuffer: 1024 * 1024 })
  console.log(`Verified thin Tauri carrier and ${names.length} root / ${instances} total Harness ${DSH_VERSION} package instances`)
}

/** Read final copied package entries, including public SDK declarations. A
 * valid source archive cannot prove that extraResources retained its exports. */
function verifyPackagedOwnedExports(profile, names = RELEASE_PACKAGES) {
  for (const name of names) {
    const directory = join(profile, 'node_modules', name)
    const manifest = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'))
    if (manifest.name !== name) throw new Error('Packaged export inventory names another package: ' + name)
    verifyBuiltPackageExports(manifest, directory)
  }
}

const args = process.argv.slice(2)
const rootFlag = args.indexOf('--root')
const installerFlag = args.indexOf('--installer')
if (rootFlag >= 0) {
  await verifyPackagedCarrier(resolve(args[rootFlag + 1]))
} else if (installerFlag >= 0) {
  const installer = resolve(args[installerFlag + 1])
  const extracted = extractInstaller(installer)
  try {
    const packagedRoot = findPackagedRoot(extracted.directory)
    if (!packagedRoot) throw new Error('No packaged quantdsh-runtime found in ' + basename(installer))
    await verifyPackagedCarrier(packagedRoot)
  } finally { extracted.cleanup() }
} else {
  throw new Error('Usage: node scripts/verify-packaged.mjs --root <dir> | --installer <path>')
}
