/** Prepare public, administrator-selected deployment configuration for one package.
 *  Faithful port of workdsh apps/desktop/scripts/prepare-deployment-config.ts. */
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export function enterpriseConnection(value) {
  const url = new URL(value)
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) {
    throw new Error('企业地址必须使用 HTTPS；仅本机测试允许 HTTP')
  }
  if (url.username || url.password || url.search || url.hash || url.pathname !== '/') {
    throw new Error('请输入企业后台地址，不包含账号、令牌或页面路径')
  }
  return { mode: 'enterprise', portalOrigin: url.origin }
}

/** Administrator-owned packaging input. Never accepts credentials or arbitrary settings. */
export function parseDeploymentConfig(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('打包配置必须是 JSON 对象')
  const root = value
  if (Object.keys(root).some(key => key !== 'enterprise')) throw new Error('打包配置仅允许 enterprise 后台地址，不接受密钥或其他设置')
  if (root.enterprise === undefined) return {}
  if (!root.enterprise || typeof root.enterprise !== 'object' || Array.isArray(root.enterprise)) throw new Error('enterprise 配置必须包含 backendUrl')
  const enterprise = root.enterprise
  if (Object.keys(enterprise).some(key => key !== 'backendUrl') || typeof enterprise.backendUrl !== 'string') {
    throw new Error('enterprise 配置仅允许 backendUrl，不接受账号或密钥')
  }
  return { enterprise: { backendUrl: enterpriseConnection(enterprise.backendUrl).portalOrigin } }
}

export function prepareDeploymentConfig(outputDirectory, inputFile) {
  const output = join(outputDirectory, 'workdsh-config.json')
  // A personal build, or a failed company build, must never reuse another
  // organization's previously generated configuration.
  rmSync(output, { force: true })
  let input = {}
  if (inputFile) {
    try { input = JSON.parse(readFileSync(inputFile, 'utf8')) } catch { throw new Error('Deployment configuration must be a readable JSON file') }
  }
  const config = parseDeploymentConfig(input)
  mkdirSync(outputDirectory, { recursive: true })
  writeFileSync(output, JSON.stringify(config, null, 2) + '\n', { mode: 0o600 })
}

export default function beforePack() {
  const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
  prepareDeploymentConfig(join(desktopRoot, 'build', 'deployment'), process.env.WORKDSH_DEPLOYMENT_CONFIG)
}

const invokedPath = process.argv[1]
if (invokedPath !== undefined && resolve(invokedPath) === fileURLToPath(import.meta.url)) beforePack()
