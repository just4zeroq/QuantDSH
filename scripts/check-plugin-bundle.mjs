/**
 * Evaluate a built plugin client bundle with a stub ModuleLoader.
 *
 * The browser bundle is CJS wrapped in `window.__ModuleLoader__.load({factory})`.
 * Loading it here runs the bundle's *module scope* (class/const initialisation,
 * helper definitions) without a DOM: enough to prove the build is not broken and
 * that `apply` is exported. Rendering still needs the host.
 */
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const plugins = process.argv.slice(2);
let failed = 0;

for (const file of plugins) {
  const source = readFileSync(file, 'utf8');
  let registered = null;
  globalThis.window = {
    __ModuleLoader__: {
      load: (mod) => {
        registered = mod;
      },
    },
  };
  // eslint-disable-next-line no-new-func
  new Function(source)();
  if (!registered) {
    console.log(`FAIL  ${file}: load() was never called`);
    failed += 1;
    continue;
  }
  const stubRequire = (id) => {
    if (id === 'react') return { createElement: () => null, useState: () => [null, () => {}] };
    if (id === 'react/jsx-runtime') return { jsx: () => null, jsxs: () => null, Fragment: null };
    throw new Error(`unexpected external import: ${id}`);
  };
  try {
    const exports = registered.factory(stubRequire);
    const names = Object.keys(exports ?? {});
    const ok = typeof exports?.apply === 'function' && typeof exports?.name === 'string';
    console.log(
      `${ok ? 'PASS' : 'FAIL'}  ${file.split(/[\\/]/).slice(-3).join('/')}  id=${registered.id}  exports=${names.join(',')}`,
    );
    if (!ok) failed += 1;
  } catch (error) {
    console.log(`FAIL  ${file}: factory threw: ${error.message}`);
    failed += 1;
  }
}

process.exit(failed === 0 ? 0 : 1);
