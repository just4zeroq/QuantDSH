// Test loader: bundle TypeScript sources with esbuild and import them.
//
// Why we write to a temp FILE instead of a `data:` URL (the pattern the pure
// domain tests use): the engine imports native/built-in modules
// (`@duckdb/node-api`, `node:sqlite`). A `data:` URL module has no path context,
// so Node cannot resolve bare specifiers from it. Emitting a real file inside the
// package lets Node walk up to `data/node_modules` and resolve them normally.

import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import fs from 'node:fs';
import path from 'node:path';

const require = createRequire(new URL('../../../../apps/web/package.json', import.meta.url));
const { build } = require('esbuild');

const srcDir = fileURLToPath(new URL('../src/', import.meta.url));
const tmpDir = fileURLToPath(new URL('./.tmp/', import.meta.url));
fs.mkdirSync(tmpDir, { recursive: true });

const cache = new Map();

/** 加载 src 下的一个模块（如 'engine/market'、'engine/sql-guard'） */
export async function loadSrc(relEntry) {
  if (cache.has(relEntry)) return cache.get(relEntry);

  const built = await build({
    stdin: {
      contents: `export * from './${relEntry}';`,
      resolveDir: srcDir,
      loader: 'ts',
    },
    bundle: true,
    platform: 'node',
    format: 'esm',
    write: false,
    // 原生模块与内置模块保持外部引用，不打包
    external: ['@duckdb/node-api', 'node:sqlite', 'node:*'],
  });

  const out = path.join(tmpDir, relEntry.replace(/[\\/]/g, '_') + '.mjs');
  fs.writeFileSync(out, built.outputFiles[0].text);
  const mod = await import(pathToFileURL(out).href);
  cache.set(relEntry, mod);
  return mod;
}
