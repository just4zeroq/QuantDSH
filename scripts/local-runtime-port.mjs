/** JS port of the local-runtime Profile helpers used only by release verification.
 *  The shipped carrier implements the same semantics in Rust (src-tauri/src/local_runtime.rs);
 *  keep both in sync. Mirrors workdsh apps/desktop/src/local-runtime.ts +
 *  runtime-compatibility.ts + plugin-archive-cache.ts. */
import { createHash, randomUUID } from 'node:crypto'
import { cpSync, existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { load, dump } from 'js-yaml'

const BASE_STATE = '.workdsh-desktop-base.json'
const json = file => JSON.parse(readFileSync(file, 'utf8'))
const writeJson = (file, value) => writeFileSync(file, JSON.stringify(value, null, 2) + '\n', { mode: 0o600 })
function memberDefaults(release) {
  return { name: 'workdsh-member-profile', private: true, type: 'module', packageManager: release.packageManager,
    dependencies: {}, ...(release.dsh ? { dsh: structuredClone(release.dsh) } : {}) }
}
function baseDigest(source, base, version) {
  const marker = join(source, '.workdsh-desktop-release.json')
  const workspace = join(source, 'pnpm-workspace.yaml')
  return createHash('sha256').update('installation-owned-defaults-v2').update(JSON.stringify(base)).update(version).update(existsSync(marker) ? readFileSync(marker) : '')
    .update(existsSync(workspace) ? readFileSync(workspace) : '').digest('hex')
}
function updatePackagePolicy(source, target) {
  const policy = join(source, 'pnpm-workspace.yaml'), current = join(target, 'pnpm-workspace.yaml')
  if (!existsSync(policy)) return false
  const parse = path => {
    const value = load(readFileSync(path, 'utf8'))
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Profile 包管理配置必须是 YAML 对象')
    return value
  }
  if (!existsSync(current)) {
    const defaults = parse(policy)
    defaults.autoInstallPeers = false
    writeFileSync(current, dump(defaults), { mode: 0o600 })
    return true
  }
  const owned = parse(policy), user = parse(current)
  if (JSON.stringify(user.overrides) === JSON.stringify(owned.overrides)) return false
  if ((user.overrides !== undefined && (!user.overrides || typeof user.overrides !== 'object' || Array.isArray(user.overrides)))
    || !owned.overrides || typeof owned.overrides !== 'object' || Array.isArray(owned.overrides)) throw new Error('Profile 包版本约束无效')
  const overrides = { ...user.overrides, ...owned.overrides }
  if (JSON.stringify(user.overrides) === JSON.stringify(overrides)) return false
  user.overrides = overrides
  writeFileSync(current, dump(user), { mode: 0o600 })
  return true
}
export function materializeRuntimeProfile(source, home) {
  const target = join(home, 'profiles', 'workdsh')
  const release = json(join(source, 'package.json'))
  const base = memberDefaults(release)
  const version = json(join(source, 'node_modules', '@deepseek-ai', 'dsh', 'package.json')).version
  if (typeof version !== 'string') throw new Error('Bundled DSH version is missing')
  const state = join(target, BASE_STATE)
  const manifest = join(target, 'package.json')
  const modules = join(target, 'node_modules')
  mkdirSync(target, { recursive: true, mode: 0o700 })
  if (existsSync(modules) && lstatSync(modules).isSymbolicLink()) throw new Error('此 Profile 的依赖仍链接到安装包，请选择独立的可写空间后再启动')
  const digest = baseDigest(source, release, version)
  const previous = existsSync(state) ? JSON.parse(readFileSync(state, 'utf8')) : undefined
  let changed = previous?.pendingInstallation === true
  if (!existsSync(manifest)) {
    writeJson(manifest, base)
    writeFileSync(join(target, 'cordis.patch.yml'), '[]\n', { mode: 0o600 })
  } else if (previous?.digest !== digest) {
    if (!previous) throw new Error('已有 Profile 缺少 Desktop 版本记录；请在官方插件管理中核对后使用独立空间')
    const current = json(manifest)
    if (current.packageManager === previous.base.packageManager) {
      if (base.packageManager === undefined) delete current.packageManager
      else current.packageManager = base.packageManager
    }
    for (const section of ['dependencies', 'optionalDependencies']) {
      const owned = previous.base[section] ?? {}
      const next = base[section] ?? {}
      current[section] ??= {}
      for (const [name, value] of Object.entries(owned)) {
        if (current[section][name] !== value) continue
        if (next[name] === undefined) delete current[section][name]
        else current[section][name] = next[name]
      }
      for (const [name, value] of Object.entries(next)) {
        if (owned[name] === undefined && current[section][name] === undefined) current[section][name] = value
      }
    }
    const selected = current.dsh?.profile?.bundles ?? []
    const oldBundles = previous.base.dsh?.profile?.bundles ?? []
    const nextBundles = base.dsh?.profile?.bundles ?? []
    current.dsh ??= {}; current.dsh.profile ??= {}
    current.dsh.profile.bundles = selected.filter(name => !oldBundles.includes(name) || nextBundles.includes(name))
    for (const name of nextBundles) if (!oldBundles.includes(name) && !current.dsh.profile.bundles.includes(name)) current.dsh.profile.bundles.push(name)
    if (JSON.stringify(json(manifest)) !== JSON.stringify(current)) {
      writeJson(manifest, current)
      changed ||= ['dependencies', 'optionalDependencies'].some(section => JSON.stringify(previous.base[section]) !== JSON.stringify(base[section])) && existsSync(modules)
    }
  }
  const policyChanged = updatePackagePolicy(source, target)
  if (policyChanged && existsSync(modules)) changed = true
  const cache = join(home, 'package-cache')
  const sourceCache = join(source, '..', '..', 'package-cache')
  if (existsSync(sourceCache)) cpSync(sourceCache, cache, { recursive: true, force: false, errorOnExist: false })
  syncBundledCompatibility(source, target, digest)
  if (changed) writeJson(state, { digest: previous?.digest ?? digest, base: previous?.base ?? base, pendingInstallation: true })
  else writeJson(state, { digest, base })
  return { profile: target, changed }
}
export function markProfileUpdated(source, profile) {
  const release = json(join(source, 'package.json'))
  const base = memberDefaults(release)
  const version = json(join(source, 'node_modules', '@deepseek-ai', 'dsh', 'package.json')).version
  const digest = baseDigest(source, release, version)
  writeJson(join(profile, BASE_STATE), { digest, base })
}
export function cachedPackageManager(home, pnpm) {
  const manifest = JSON.parse(readFileSync(join(dirname(pnpm), '..', 'package.json'), 'utf8'))
  if (manifest.name !== 'pnpm' || typeof manifest.version !== 'string' || !/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/u.test(manifest.version)) {
    throw new Error('内置插件安装工具清单无效')
  }
  const fixedManager = Number(manifest.version.split('.')[0]) >= 11
    ? ['--pm-on-fail=ignore', '--config.verify-deps-before-run=false']
    : ['--config.manage-package-manager-versions=false', '--config.package-manager-strict=false']
  const runner = join(home, '.workdsh-package-manager.mjs')
  writeFileSync(runner, String.raw`import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
const cache = join(fileURLToPath(new URL('.', import.meta.url)), 'plugin-archives');
const args = process.argv.slice(2);
if (args.includes('add')) {
  for (let i = 0; i < args.length; i++) {
    const spec = args[i];
    if (!/\.(tgz|tar\.gz)$/i.test(spec) || /^(https?:|npm:)/i.test(spec)) continue;
    const source = spec.startsWith('file://') ? fileURLToPath(spec) : resolve(spec.startsWith('file:') ? spec.slice(5) : spec);
    if (!statSync(source).isFile()) throw new Error('Plugin archive is not a file: ' + source);
    const bytes = readFileSync(source);
    const digest = createHash('sha256').update(bytes).digest('hex');
    mkdirSync(cache, { recursive: true, mode: 0o700 });
    const destination = join(cache, digest + '.tgz');
    try { writeFileSync(destination, bytes, { flag: 'wx', mode: 0o600 }); }
    catch (error) { if (error.code !== 'EEXIST') throw error; }
    if (createHash('sha256').update(readFileSync(destination)).digest('hex') !== digest) throw new Error('Plugin archive cache integrity failure');
    args[i] = destination;
  }
}
const child = spawn(process.execPath, [${JSON.stringify(pnpm)}, ...${JSON.stringify(fixedManager)}, ...args], { stdio: 'inherit', windowsHide: true });
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => child.kill(signal));
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', (code, signal) => { process.exitCode = code ?? (signal ? 1 : 0); });
`, { mode: 0o600 })
  return runner
}
/** The official application-owned Profile API supplies the full installation dependency graph. */
export function officialHostLauncher(source, home, executable, packageManager, patches) {
  const boot = join(source, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'profile-boot.js')
  const appBoot = join(source, 'node_modules', '@deepseek-ai', 'dsh-app-boot', 'lib', 'index.js')
  if (!existsSync(boot) || !existsSync(appBoot) || !existsSync(packageManager)) throw new Error('安装包缺少官方 Profile 启动接口')
  const anchor = join(source, 'profile-installation.json'), profile = join(home, 'profiles', 'workdsh')
  if (!existsSync(anchor)) throw new Error('安装包缺少完整官方与 WorkDSH 安装清单')
  const launcher = join(home, '.workdsh-host.mjs')
  packageManager = cachedPackageManager(home, packageManager)
  writeFileSync(launcher, `import { runProfile } from ${JSON.stringify(pathToFileURL(boot).href)};\nimport { loadProfileDirectory, loadLayeredEnv, reportSkippedBundles } from ${JSON.stringify(pathToFileURL(appBoot).href)};\nconst installAnchor = ${JSON.stringify(anchor)};\nconst profile = loadProfileDirectory('dsh', ${JSON.stringify(profile)}, installAnchor);\nreportSkippedBundles('dsh', profile);\nawait runProfile({ environment: loadLayeredEnv('dsh'), profile: 'workdsh', resolvedProfile: { profile, installAnchor }, patchFiles: ${JSON.stringify(patches)}, args: process.argv.slice(2), packageManager: { command: ${JSON.stringify(executable)}, args: [${JSON.stringify(packageManager)}] } });\n`, { mode: 0o600 })
  return launcher
}

/** Carry bundled exact-version grants forward without replacing user grants. */
const PACKAGE_VERSION = /^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*@(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+)?$/u
const EXACT_VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+)?$/u
function readGrants(filename) {
  const parsed = JSON.parse(readFileSync(filename, 'utf8'))
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error(`${filename} must contain an object of exact-version grants`)
  const grants = {}
  for (const [key, versions] of Object.entries(parsed)) {
    if (!PACKAGE_VERSION.test(key) || !Array.isArray(versions) || !versions.every(version => typeof version === 'string' && EXACT_VERSION.test(version))) {
      throw new Error(`${filename} has an invalid grant for ${key}`)
    }
    grants[key] = versions
  }
  return grants
}
function writeGrants(filename, grants) {
  const temporary = `${filename}.${process.pid}.${randomUUID()}.tmp`
  try {
    writeFileSync(temporary, `${JSON.stringify(grants, null, 2)}\n`, { mode: 0o600 })
    renameSync(temporary, filename)
  } finally { rmSync(temporary, { force: true }) }
}
export function syncBundledCompatibility(sourceProfile, targetProfile, runtimeVersion) {
  const marker = join(targetProfile, '.workdsh-desktop-compatibility')
  const previousBundledFile = join(targetProfile, '.workdsh-desktop-bundled-grants.json')
  if (existsSync(marker) && readFileSync(marker, 'utf8').trim() === runtimeVersion) return
  const source = join(sourceProfile, 'compatibility.json')
  const target = join(targetProfile, 'compatibility.json')
  const bundled = existsSync(source) ? readGrants(source) : {}
  let current = {}
  let previousBundled = {}
  try {
    if (existsSync(target)) current = readGrants(target)
    if (existsSync(previousBundledFile)) previousBundled = readGrants(previousBundledFile)
  } catch (error) {
    console.warn(`WorkDSH cannot update ${target}; repair its compatibility metadata to receive bundled plugin grants: ${String(error)}`)
    return
  }
  for (const [key, versions] of Object.entries(bundled)) {
    const previous = new Set(previousBundled[key] ?? [])
    const granted = new Set(current[key] ?? [])
    for (const version of versions) {
      if (!previous.has(version)) granted.add(version)
    }
    if (granted.size) current[key] = [...granted]
  }
  writeGrants(target, current)
  writeGrants(previousBundledFile, bundled)
  writeFileSync(marker, `${runtimeVersion}\n`)
}
