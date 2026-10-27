import { build } from 'esbuild';
import { readFile, mkdir, copyFile, writeFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';

// Mirrors build-data.mjs. Host entry is bundled as Node ESM with all deps
// external; the client (the Vela chart page) is bundled for the browser as CJS
// and wrapped in the platform ModuleLoader factory. @luxalgo/vela is bundled
// INTO the client (not external) so the chart renders without a separate loader.
const root = new URL('../../../packages/plugins/market/', import.meta.url);
const manifest = JSON.parse(await readFile(new URL('package.json', root), 'utf8'));
const external = Object.keys({ ...manifest.dependencies, ...manifest.peerDependencies });

// 1) Host entry — Node ESM, deps external.
await build({
  entryPoints: [fileURLToPath(new URL('src/index.ts', root))],
  outfile: fileURLToPath(new URL('dist/index.js', root)),
  bundle: true,
  platform: 'node',
  format: 'esm',
  external,
});

// 2) Browser client — CJS, wrapped in window.__ModuleLoader__.
//    react / react/jsx-runtime / the workbenches client are provided by the host;
//    @luxalgo/vela is bundled in.
const client = await build({
  entryPoints: [fileURLToPath(new URL('src/client.tsx', root))],
  bundle: true,
  write: false,
  platform: 'browser',
  format: 'cjs',
  target: 'es2022',
  external: ['react', 'react/jsx-runtime', 'quantdsh-plugin-workbenches/client'],
});
await writeFile(
  new URL('dist/client.browser.js', root),
  `window.__ModuleLoader__.load({id:"quantdsh-plugin-market",factory:function(require){const module={exports:{}};\n${client.outputFiles[0].text}\nreturn module.exports;}});\n`,
);

// 3) Attribution: Vela™ is Apache-2.0 with a NOTICE attribution requirement.
//    The chart renders its watermark by default (kept on), and we also ship the
//    license text so redistribution stays compliant with Apache-2.0 §4(d).
await copyVelaLicense(root);

async function copyVelaLicense(root) {
  const dest = fileURLToPath(new URL('dist/THIRD-PARTY-LICENSES.txt', root));
  const candidates = [
    new URL('../../../node_modules/@luxalgo/vela/LICENSE', import.meta.url),
    new URL('../../../packages/plugins/market/node_modules/@luxalgo/vela/LICENSE', import.meta.url),
    new URL('../../../node_modules/@luxalgo/vela/NOTICE', import.meta.url),
    new URL('../../../packages/plugins/market/node_modules/@luxalgo/vela/NOTICE', import.meta.url),
  ];
  const parts = [
    'QuantDSH market workbench bundles LuxAlgo Vela™ (https://velacharts.dev), licensed under Apache-2.0.\n',
    'Vela renders a small attribution watermark by default; this satisfies the NOTICE attribution requirement.\n',
  ];
  for (const url of candidates) {
    try {
      const text = await readFile(url, 'utf8');
      const name = url.pathname.endsWith('NOTICE') ? 'Vela NOTICE' : 'Vela LICENSE';
      parts.push(`\n===== ${name} =====\n${text}`);
    } catch {
      /* file not present in this tree; skip */
    }
  }
  if (parts.length > 2) await writeFile(dest, parts.join('\n'));
}
