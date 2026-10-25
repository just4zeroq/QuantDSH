/** Generate the Tauri icon set from the repository-owned cross-platform artwork.
 *  Mirrors workdsh build/app-icon.png (1024x1024 RGBA16) and build/app-icon-mac.png,
 *  which are produced by the upstream generate-windows-app-icon.mjs /
 *  generate-mac-app-icon.mjs / generate-tray-icons.mjs pipeline. */

import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import sharp from 'sharp'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const assets = join(root, 'assets')
const icons = join(root, 'src-tauri', 'icons')

const MAC_SOURCE = join(assets, 'app-icon-mac.png')
const WIN_SOURCE = join(assets, 'app-icon.png')

async function generatePng(source, output, size) {
  const rendered = await sharp(source, { failOn: 'warning' })
    .resize({ width: size, height: size, fit: 'fill', kernel: sharp.kernel.lanczos3 })
    .toColourspace('srgb')
    .ensureAlpha()
    .png({ compressionLevel: 9, progressive: false, adaptiveFiltering: false, palette: false })
    .toBuffer()
  await writeFile(join(icons, output), rendered)
  console.log(`Generated ${output} (${size}x${size})`)
}

async function generateIcns() {
  // Tauri's icon.icns: is32/is16 (icp4/icp5 legacy), ic07..ic04 retina variants.
  // Build the modern variants; the 32/16 frames reuse the resized PNG payloads.
  const buffer = await readFile(MAC_SOURCE)
  const sizes = [
    ['icp4', 16], ['icp5', 32], ['icp6', 64], ['ic07', 128], ['ic08', 256],
    ['ic09', 512], ['ic10', 1024], ['ic11', 32], ['ic12', 64], ['ic13', 256], ['ic14', 512],
  ]
  const entries = []
  for (const [type, size] of sizes) {
    const png = await sharp(buffer, { failOn: 'warning' })
      .resize({ width: size, height: size, fit: 'fill', kernel: sharp.kernel.lanczos3 })
      .toColourspace('srgb')
      .ensureAlpha()
      .png({ compressionLevel: 9, progressive: false, adaptiveFiltering: false, palette: false })
      .toBuffer()
    entries.push({ type, png })
  }
  // ICNS header: magic 'icns' + total length; each entry: type + (length - 8) + PNG data.
  const headerLength = 8
  const entryLength = entries.reduce((total, entry) => total + 8 + entry.png.length, 0)
  const out = Buffer.alloc(headerLength + entryLength)
  out.write('icns', 0, 'ascii')
  out.writeUInt32BE(headerLength + entryLength, 4)
  let offset = headerLength
  for (const entry of entries) {
    out.write(entry.type, offset, 'ascii')
    out.writeUInt32BE(8 + entry.png.length, offset + 4)
    entry.png.copy(out, offset + 8)
    offset += 8 + entry.png.length
  }
  await writeFile(join(icons, 'icon.icns'), out)
  console.log(`Generated icon.icns (${out.length} bytes)`)
}

await mkdir(icons, { recursive: true })
// Window/taskbar icons: canonical square artwork at native sizes.
await generatePng(WIN_SOURCE, '32x32.png', 32)
await generatePng(WIN_SOURCE, '64x64.png', 64)
await generatePng(WIN_SOURCE, '128x128.png', 128)
await generatePng(WIN_SOURCE, '128x128@2x.png', 256)
// Store icon: same artwork, one canonical store square.
await generatePng(WIN_SOURCE, 'icon.png', 512)
// Windows ICO is copied verbatim from the upstream build (exact-DPI frames).
await writeFile(join(icons, 'icon.ico'), await readFile(join(assets, 'app-icon.ico')))
console.log('Copied icon.ico from assets/app-icon.ico')
// macOS: Dock-safe artwork with the platform visual inset.
await generateIcns()
