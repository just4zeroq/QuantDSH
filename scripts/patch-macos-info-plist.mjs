#!/usr/bin/env node
/** Apply electron-builder's `build.mac.extendInfo` to the bundled macOS Info.plist.
 *
 *  Tauri has no `extendInfo` equivalent, so the same keys are written after
 *  `tauri build` produced the `.app`. Source of truth:
 *  workdsh/apps/desktop/package.json → build.mac.extendInfo.
 *
 *  `LSUIElement: true` is what makes the bundle an agent process at launch. The
 *  carrier then calls `setActivationPolicy(Regular)` (the Electron main process
 *  does the same with `app.setActivationPolicy('regular')`), so the primary app
 *  shows a Dock icon while a spawned `--quantdsh-browser-worker-*` child — the
 *  same executable — never flashes one before JavaScript runs.
 *
 *  Usage: node scripts/patch-macos-info-plist.mjs --app <path to QuantDSH.app>
 */
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join, resolve } from 'node:path'

const args = process.argv.slice(2)
const appFlag = args.indexOf('--app')
if (appFlag < 0) throw new Error('Usage: node scripts/patch-macos-info-plist.mjs --app <path to QuantDSH.app>')

const app = resolve(args[appFlag + 1])
const plist = join(app, 'Contents', 'Info.plist')
if (!existsSync(plist)) throw new Error(`No Info.plist in ${app}`)

function plutil(plutilArgs) {
  return spawnSync('plutil', [...plutilArgs, plist], { encoding: 'utf8' })
}

/** `-replace` refuses to create keys, so drop the key first and ignore "not found". */
function write(key, type, value) {
  plutil(['-remove', key])
  const result = plutil(['-insert', key, type, value])
  if (result.status !== 0) throw new Error(`plutil -insert ${key} failed: ${result.stderr || result.stdout}`)
}

write('LSUIElement', '-bool', 'YES')
write('CFBundleAllowMixedLocalizations', '-bool', 'YES')
write('CFBundleDevelopmentRegion', '-string', 'en')
write('CFBundleLocalizations', '-json', '["en","zh_CN"]')

console.log(`Patched ${plist}: LSUIElement, CFBundleAllowMixedLocalizations, CFBundleDevelopmentRegion, CFBundleLocalizations`)
