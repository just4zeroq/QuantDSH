import { readFile, realpath } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { InstalledWorkbenchManifestReader } from './installed-manifest-reader.js';
import { inspectWorkbenchPackageArtifact } from './package-artifact.js';
import { verifyInstalledWorkbenchArtifact } from './installed-artifact.js';
import type { InstalledWorkbenchManifest } from './manifest.js';

/** Anchor-installed workbench facts. Default product bundles live in the
 * immutable installation anchor (the packer's Profile project), never in the
 * writable Profile: `verify-installation-anchor` asserts the writable space
 * stays dependency-free. Their authority is the installer-written release
 * manifest in the anchor's package-cache plus the anchor's own node_modules
 * bytes — observed facts, never page-supplied values. */
export interface AnchorInstallation {
  readonly root: string;
  readonly manifest: InstalledWorkbenchManifest;
}

interface ReleasePackageEntry {
  readonly name: string;
  readonly version: string;
  readonly filename: string;
  readonly sha256: string;
}

/** `realpath` failures distinguish "not installed here" (fall through) from
 * tampering (propagate). Only ENOENT/ENOTDIR qualify as absence. */
export function isPackageAbsence(error: unknown): boolean {
  const code = (error as NodeJS.ErrnoException | undefined)?.code;
  return code === 'ENOENT' || code === 'ENOTDIR';
}

async function releasePackageIndex(anchorRoot: string): Promise<Map<string, ReleasePackageEntry>> {
  // The anchor Profile project sits at <runtime>/profiles/quantdsh; the installer
  // writes the package cache at <runtime>/package-cache with its release manifest.
  const index = join(anchorRoot, '..', '..', 'package-cache', 'release-manifest.json');
  const data = JSON.parse(await readFile(index, 'utf8')) as { packages?: readonly ReleasePackageEntry[] };
  if (!Array.isArray(data.packages)) throw new Error('workbench/invalid-release-manifest');
  const index_ = new Map<string, ReleasePackageEntry>();
  for (const row of data.packages) {
    if (row && typeof row.name === 'string' && typeof row.version === 'string'
      && typeof row.filename === 'string' && typeof row.sha256 === 'string') index_.set(row.name, row);
  }
  return index_;
}

/** The writable Profile derives its anchor from the Host adapter (`profile.installAnchor`,
 * an absolute file path inside the anchor Profile project). Returns undefined when the
 * Host supplies no anchor so callers keep their previous behavior unchanged. */
export async function resolveAnchorInstallation(installAnchor: string | undefined, packageName: string,
  signal?: AbortSignal): Promise<AnchorInstallation | undefined> {
  signal?.throwIfAborted();
  if (typeof installAnchor !== 'string' || !installAnchor) return undefined;
  const root = await realpath(dirname(installAnchor));
  const manifest = await new InstalledWorkbenchManifestReader(root).read(packageName, signal);
  if (!manifest) return undefined;
  return Object.freeze({ root, manifest });
}

/** Verify an anchor-installed workbench against its released archive: the release
 * manifest pins the archive hash, the archive pins its own manifest, and the
 * archive pins the anchor's installed bytes — the same three-way proof the
 * writable Profile performs, with anchor facts substituted for installation
 * facts. Never writes into the writable Profile's installation journal. */
export async function verifyAnchorInstallation(anchor: AnchorInstallation,
  isCompatible: (descriptor: AnchorInstallation['manifest']['descriptor']) => boolean,
  signal?: AbortSignal) {
  signal?.throwIfAborted();
  const index = await releasePackageIndex(anchor.root);
  const entry = index.get(anchor.manifest.packageName);
  if (!entry || entry.version !== anchor.manifest.version) throw new Error('workbench/installation-changed');
  const archivePath = join(anchor.root, '..', '..', 'package-cache', entry.filename);
  const expected = {
    packageName: anchor.manifest.packageName,
    version: anchor.manifest.version,
    workbenchId: anchor.manifest.descriptor.id,
    sha256: entry.sha256,
  };
  // Hash-check the released archive against the release manifest…
  const artifact = await inspectWorkbenchPackageArtifact(archivePath, expected, signal);
  // …then prove the anchor's installed bytes still match that archive.
  await verifyInstalledWorkbenchArtifact(anchor.root, archivePath, expected, signal);
  return Object.freeze({
    package: artifact.receipt,
    descriptor: artifact.manifest.descriptor,
    compatible: isCompatible(artifact.manifest.descriptor),
  });
}
