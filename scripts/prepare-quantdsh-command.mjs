import { mkdirSync, copyFileSync, writeFileSync } from 'node:fs'
import { cpSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { prepareOfficialRelease } from './official-desktop-release.mjs'
const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const target = process.platform === 'win32' ? 'win-x64' : `mac-${process.env.QUANTDSH_MAC_ARCH ?? process.arch}`
const released = await prepareOfficialRelease(desktopRoot, target)
const destination = resolve(desktopRoot, 'build/quantdsh-runtime/runtime/cli')
cpSync(join(released, 'runtime/cli'), destination, { recursive: true, force: true, dereference: false })
mkdirSync(join(destination, 'bin'), { recursive: true })
copyFileSync(resolve(desktopRoot, 'runtime-cli/command-cli.mjs'), join(destination, 'command-cli.js'))
writeFileSync(join(destination, 'bin/dsh'), `#!/bin/sh
set -e
launcher=$0
while [ -L "$launcher" ]; do
  directory=$(CDPATH= cd -- "$(dirname -- "$launcher")" && pwd -P)
  target=$(readlink "$launcher")
  case "$target" in /*) launcher=$target ;; *) launcher=$directory/$target ;; esac
done
runtime=$(CDPATH= cd -- "$(dirname -- "$launcher")/../../.." && pwd -P)
exec "$runtime/primary-runtime/dependencies/node/bin/node" "$runtime/runtime/cli/command-cli.js" "$@"
`, { mode: 0o755 })
writeFileSync(join(destination, 'bin/dsh.cmd'), '@echo off\r\n"%~dp0..\\..\\..\\primary-runtime\\dependencies\\node\\bin\\node.exe" "%~dp0..\\command-cli.js" %*\r\nexit /b %errorlevel%\r\n')
