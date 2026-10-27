import { build } from 'esbuild';
import { readFile, mkdir, readdir, copyFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, relative, join } from 'node:path';

// Mirrors build-quality-analysis.mjs. Host entries are bundled as Node ESM with
// all deps external; the client is bundled for the browser and wrapped in the
// platform ModuleLoader factory. A `.domain` subpath bundle is emitted so the
// market (quote) page can import the shared domain layer without pulling the
// full data workbench.
const root = new URL('../../../packages/plugins/data/', import.meta.url);
const manifest = JSON.parse(await readFile(new URL('package.json', root), 'utf8'));
const external = Object.keys({ ...manifest.dependencies, ...manifest.peerDependencies });

await copyFile(
  new URL('../../../packages/tabular/dist/THIRD-PARTY-LICENSES.txt', import.meta.url),
  new URL('dist/THIRD-PARTY-LICENSES.txt', root),
);

for (const name of ['index', 'domain/index']) {
  const outfile = name === 'index' ? 'index' : 'domain';
  await build({
    entryPoints: [fileURLToPath(new URL('src/' + name + '.ts', root))],
    outfile: fileURLToPath(new URL('dist/' + outfile + '.js', root)),
    bundle: true,
    platform: 'node',
    format: 'esm',
    external,
  });
}

const dist = fileURLToPath(new URL('dist/', root));
const destination = join(dist, 'contracts');
await mkdir(destination, { recursive: true });
for (const name of await readdir(new URL('../../../packages/contracts/dist/', import.meta.url))) {
  if (name.endsWith('.d.ts')) await copyFile(new URL('../../../packages/contracts/dist/' + name, import.meta.url), join(destination, name));
}
async function rewrite(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (path !== destination) await rewrite(path);
      continue;
    }
    if (!entry.name.endsWith('.d.ts')) continue;
    let target = relative(dirname(path), join(destination, 'index.js')).replaceAll('\\', '/');
    if (!target.startsWith('.')) target = './' + target;
    await writeFile(path, (await readFile(path, 'utf8')).replace(/(['"])quantdsh-contracts\1/g, JSON.stringify(target)));
  }
}
await rewrite(dist);

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
  `window.__ModuleLoader__.load({id:"quantdsh-plugin-data",factory:function(require){const module={exports:{}};\n${client.outputFiles[0].text}\nreturn module.exports;}});\n`,
);
