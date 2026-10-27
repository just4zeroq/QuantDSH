#!/usr/bin/env node
/**
 * Prepare the CPython runtime that ships inside the installer.
 *
 * The data service is a FastAPI app, so the bundle needs an interpreter that
 * exists on the user's machine exactly once: the one we ship. This script
 * downloads a relocatable CPython (python-build-standalone), unpacks it to
 * `build/python-runtime/python`, and installs `services/datahub-api/requirements.txt`
 * into it.
 *
 * `tauri.conf.json` maps `build/python-runtime` to `runtime/`, so the carrier
 * starts `runtime/python/python.exe -m app` with `PYTHONUTF8=1` — no system
 * Python is ever consulted (`-s` disables user site-packages).
 *
 * Idempotent: re-running is a no-op once the interpreter imports every wheel.
 * Override the download with PYTHON_RUNTIME_URL when the network cannot reach
 * GitHub.
 */
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..');
const OUT = path.join(ROOT, 'build', 'python-runtime');
const PY = path.join(OUT, 'python');
const CACHE = path.join(ROOT, 'build', '.cache');
const REQ = path.join(ROOT, 'services', 'datahub-api', 'requirements.txt');
const IS_WIN = process.platform === 'win32';

const TRIPLES = {
  'win32-x64': 'x86_64-pc-windows-msvc',
  'win32-arm64': 'aarch64-pc-windows-msvc',
  'darwin-x64': 'x86_64-apple-darwin',
  'darwin-arm64': 'aarch64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
};

const triple = TRIPLES[`${process.platform}-${process.arch}`];
if (!triple) {
  console.error(`[python-runtime] unsupported platform ${process.platform}-${process.arch}`);
  process.exit(1);
}

const pythonExe = (root) =>
  IS_WIN ? path.join(root, 'python.exe') : path.join(root, 'bin', 'python3');

function checkRuntime() {
  const exe = pythonExe(PY);
  if (!fs.existsSync(exe)) return false;
  try {
    const out = execFileSync(exe, ['-c', 'import fastapi, uvicorn, ccxt, duckdb; print(duckdb.__version__)'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
    console.log(`[python-runtime] ready (duckdb ${out})`);
    return true;
  } catch (error) {
    console.log(`[python-runtime] interpreter present but imports failed: ${error.message.split('\n')[0]}`);
    return false;
  }
}

function pickAsset() {
  if (process.env.PYTHON_RUNTIME_URL) {
    return { url: process.env.PYTHON_RUNTIME_URL, name: path.basename(process.env.PYTHON_RUNTIME_URL) };
  }
  // `releases/latest` instead of the release list: the list endpoint returns
  // >10 MB of asset metadata and times out on a slow link. The payload is
  // matched with a regex rather than parsed — same one fact is needed, and a
  // 10 MB JSON.parse buys nothing.
  const json = execFileSync(
    'curl',
    ['-sSL', '--compressed', '--max-time', '300', 'https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest'],
    { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 },
  );
  const pattern = new RegExp(
    `https://[^"\\\\]+cpython-3\\.1[23]\\.\\d+(?:\\+|%2B)\\d+-${triple}-install_only\\.tar\\.gz`,
  );
  const match = json.match(pattern);
  if (!match) {
    throw new Error(
      `no cpython 3.1x install_only asset for ${triple}; response starts with: ${json.slice(0, 200)}`,
    );
  }
  return { url: match[0].replace('%2B', '+'), name: match[0].split('/').pop().replace('%2B', '+') };
}

/**
 * Delete what a shipped interpreter never reads.
 *
 * python-build-standalone ships MSVC debug symbols next to every binary — 83 MB
 * of `.pdb` for a 291 MB tree — plus headers and import libraries for building
 * C extensions. The bundle only runs `.py`, so none of it is load-bearing.
 * Runs on every invocation, including the idempotent one, so an already-prepared
 * tree gets trimmed too.
 */
function prune() {
  // Measure the tree instead of summing deleted files: `statSync` on the
  // bundled `.pdb` files reports a fraction of their on-disk footprint (they are
  // written sparse), so a per-file tally under-reports the real saving.
  const before = dirSize(OUT);
  const walk = dir => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.toLowerCase().endsWith('.pdb')) fs.rmSync(full, { force: true });
    }
  };
  walk(OUT);
  for (const dir of [path.join(PY, 'include'), path.join(PY, 'libs')]) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  const saved = before - dirSize(OUT);
  if (saved > 0) console.log(`[python-runtime] pruned ${(saved / 1024 / 1024).toFixed(0)} MB of debug symbols and headers`);
}

function main() {
  if (checkRuntime()) {
    prune();
    return;
  }
  fs.mkdirSync(CACHE, { recursive: true });
  fs.mkdirSync(OUT, { recursive: true });

  const asset = pickAsset();
  const tarball = path.join(CACHE, asset.name);
  if (!fs.existsSync(tarball) || fs.statSync(tarball).size === 0) {
    console.log(`[python-runtime] downloading ${asset.name}`);
    execFileSync('curl', ['-L', '--fail', '--max-time', '900', '-o', tarball, asset.url], { stdio: 'inherit' });
  }

  console.log('[python-runtime] extracting');
  execFileSync('tar', ['-xzf', tarball, '-C', OUT], { stdio: 'inherit' });

  const exe = pythonExe(PY);
  if (!fs.existsSync(exe)) throw new Error(`interpreter not found after extract: ${exe}`);

  console.log('[python-runtime] installing wheels');
  execFileSync(exe, ['-m', 'pip', 'install', '--disable-pip-version-check', '--no-warn-script-location', '-q', '-r', REQ], {
    stdio: 'inherit',
  });

  prune();

  if (!checkRuntime()) throw new Error('runtime verification failed after install');
  console.log(`[python-runtime] size ${(dirSize(OUT) / 1024 / 1024).toFixed(0)} MB`);
}

function dirSize(dir) {
  let total = 0;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    total += entry.isDirectory() ? dirSize(full) : fs.statSync(full).size;
  }
  return total;
}

main();
