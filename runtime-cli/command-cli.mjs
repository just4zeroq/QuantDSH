#!/usr/bin/env node
/** The installed command delegates to the official CLI in the selected account space.
 *  Faithful CommonJS port of the upstream desktop's command-cli.ts. */
'use strict'

const { readFileSync, existsSync } = require('node:fs')
const { homedir } = require('node:os')
const { dirname, join, isAbsolute } = require('node:path')
const { spawn } = require('node:child_process')

function enterpriseEnvironment(root, executable, parent) {
  const result = {}
  for (const key of ['SystemRoot', 'WINDIR', 'ComSpec', 'PATHEXT', 'LANG', 'LC_ALL', 'TZ', 'DISPLAY', 'WAYLAND_DISPLAY']) {
    if (parent[key]) result[key] = parent[key]
  }
  result.PATH = `${dirname(executable)}${process.platform === 'win32' ? ';' : ':'}${parent.PATH ?? ''}`
  result.HOME = join(root, 'home'); result.USERPROFILE = result.HOME
  result.DSH_HOME = join(root, 'dsh'); result.DSH_AGENTS_HOME = join(root, 'agents')
  result.TMPDIR = join(root, 'tmp'); result.TMP = result.TMPDIR; result.TEMP = result.TMPDIR
  result.APPDATA = join(root, 'appdata'); result.LOCALAPPDATA = join(root, 'localappdata')
  return result
}

function selectCommandSpace(context, request) {
  if (!isAbsolute(context.personalHome)) throw new Error('Invalid personal command space')
  if (request !== undefined && !['personal', 'enterprise'].includes(request)) throw new Error('请选择 personal 或 enterprise')
  const space = request === 'personal' || (!request && !context.active)
    ? { mode: 'personal', home: context.personalHome, label: '个人工作区' }
    : context.active
  if (!space || !isAbsolute(space.home) || !['personal', 'enterprise'].includes(space.mode) || (request === 'enterprise' && space.mode !== 'enterprise')) throw new Error('请先在 Desktop 登录企业账号')
  if (space.mode === 'enterprise' && (!space.root || !isAbsolute(space.root) || space.home !== join(space.root, 'dsh'))) throw new Error('Invalid enterprise command space')
  return space
}

async function main() {
  const userData = process.env.QUANTDSH_DESKTOP_USER_DATA ?? (process.platform === 'win32'
    ? join(process.env.APPDATA ?? '', 'QuantDSH')
    : join(homedir(), 'Library/Application Support/QuantDSH'))
  const context = JSON.parse(readFileSync(join(userData, 'command-context.json'), 'utf8'))
  const args = process.argv.slice(2)
  const spaceOption = args.findIndex(value => value.startsWith('--quantdsh-space='))
  const request = spaceOption < 0 ? undefined : args.splice(spaceOption, 1)[0].slice('--quantdsh-space='.length)
  const space = selectCommandSpace(context, request)
  const launcher = join(space.home, '.quantdsh-launch.mjs')
  const patch = join(space.home, '.enterprise-desktop.patch.yml')
  if (space.mode === 'enterprise' && (!existsSync(join(space.home, 'enterprise-auth.json')) || !existsSync(patch))) throw new Error('企业账号已退出，请重新登录 Desktop')
  if (!existsSync(launcher)) throw new Error('请先在 Desktop 打开此工作区，完成官方 Profile 初始化')
  const runtime = dirname(dirname(dirname(__filename)))
  const primary = join(runtime, 'primary-runtime')
  const basePatch = join(runtime, 'profiles/quantdsh/cordis.patch.yml')
  const patches = [...(existsSync(basePatch) ? ['--patch', basePatch] : []), ...(space.mode === 'enterprise' ? ['--patch', patch] : [])]
  const env = space.mode === 'enterprise' ? enterpriseEnvironment(space.root, process.execPath, process.env) : { ...process.env }
  Object.assign(env, {
    DSH_HOME: space.home,
    DSH_AGENTS_HOME: space.mode === 'enterprise' ? join(space.root, 'agents') : join(space.home, 'agents'),
    DSH_BUNDLED_PRIMARY_RUNTIME: primary,
    ELECTRON_RUN_AS_NODE: undefined,
  })
  process.stderr.write(`QuantDSH：${space.label}\n`)
  const child = spawn(process.execPath, [launcher, '--profile', 'quantdsh', ...patches, ...args], { cwd: space.home, env, stdio: 'inherit' })
  child.once('error', error => { process.stderr.write(error.message + '\n'); process.exitCode = 1 })
  child.once('exit', code => { process.exitCode = code ?? 1 })
}

if (require.main === module) {
  main().catch(error => { process.stderr.write(String(error instanceof Error ? error.message : error) + '\n'); process.exitCode = 1 })
}

module.exports = { selectCommandSpace, enterpriseEnvironment }
