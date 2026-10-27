import { bootstrapInstalledWorkbenchArchives } from './bootstrap-installed-archives.js';
import { createHash } from 'node:crypto';
import { realpath } from 'node:fs/promises';
import { isAbsolute, dirname } from 'node:path';
import type { Context } from '@deepseek-ai/cordis';
import type { ProfileContext } from '@deepseek-ai/dsh-app-boot';
import type { DomainFacility } from '@deepseek-ai/dsh-storage-domain';
import type { HostConnectionHandle } from '@deepseek-ai/dsh-client-connection';
import { assertActorContext, type IdentityService } from 'quantdsh-contracts';
import { handleWorkbenchRegistrationRequest, workbenchRegistrationPath } from './registration-rpc.js';
import type { WorkbenchDescriptor } from 'quantdsh-contracts';
import { WorkbenchInstallationArchives } from './installation-archives.js';
import { InstalledWorkbenchManifestReader } from './installed-manifest-reader.js';
import { createWorkbenchInstallationVerifier } from './installation-verifier.js';
import { HostWorkbenchRegistration } from './host-registration.js';
import type { WorkbenchLoader } from './loader-source.js';
import { isPackageAbsence, resolveAnchorInstallation, verifyAnchorInstallation } from './anchor-installation.js';
import type { WorkbenchManifestReader } from './catalog.js';

/** The writable Profile owns user-installed workbenches; default product bundles
 * are anchor-installed and stay absent from the writable space by design. Reads
 * fall through to the anchor only on package absence — a tampered writable
 * installation still fails loudly. */
class AnchorFallbackManifestReader implements WorkbenchManifestReader {
  constructor(private readonly profile: WorkbenchManifestReader, private readonly anchor?: WorkbenchManifestReader) {}
  async read(packageName: string, signal?: AbortSignal) {
    try { return await this.profile.read(packageName, signal); }
    catch (cause) {
      if (!this.anchor || !isPackageAbsence(cause)) throw cause;
      return this.anchor.read(packageName, signal);
    }
  }
}

/** Internal platform composition. The actual official launcher supplies the
 * Profile directory; configuration and RPC cannot select another installation. */
export async function mountWorkbenchRegistrationHost<Page>(ctx:Context, isCompatible:(descriptor:WorkbenchDescriptor)=>boolean) {
  const profile=ctx.get('profileContext') as ProfileContext | undefined;
  const storage=ctx.get('storageDomain') as DomainFacility | undefined;
  const loader=ctx.get('loader') as WorkbenchLoader | undefined;
  if(!profile || !storage || !loader || !isAbsolute(profile.dir))throw new Error('workbench/host-services-unavailable');
  ctx.fiber.assertActive();
  const directory=await realpath(profile.dir);
  const installationId=createHash('sha256').update(JSON.stringify([profile.name,directory])).digest('hex');
  const archives=await WorkbenchInstallationArchives.open(storage,installationId,directory);
  try {
    ctx.fiber.assertActive();
    await bootstrapInstalledWorkbenchArchives(directory,archives,()=>ctx.fiber.assertActive());
    // Anchor support: the launcher-provided installAnchor file lives inside the
    // anchor Profile project (its dirname is the anchor root).
    const anchorFile = (profile as ProfileContext & { installAnchor?: unknown }).installAnchor;
    const anchorInstallAnchor = typeof anchorFile === 'string' && anchorFile ? anchorFile : undefined;
    const anchorReader = anchorInstallAnchor
      ? new InstalledWorkbenchManifestReader(await realpath(dirname(anchorInstallAnchor)))
      : undefined;
    const reader = anchorReader ? new AnchorFallbackManifestReader(new InstalledWorkbenchManifestReader(directory), anchorReader) : new InstalledWorkbenchManifestReader(directory);
    const localVerifier = createWorkbenchInstallationVerifier(directory,async(name,version)=>archives.resolve(name,version),isCompatible);
    const verifier = anchorInstallAnchor ? async (name: string, version: string, signal?: AbortSignal) => {
      try { return await localVerifier(name, version, signal); }
      catch (cause) {
        // Only genuine "no retained archive for this package" falls through to
        // the anchor; version conflicts and journal mismatches still fail.
        if (!(cause instanceof Error) || cause.message !== 'workbench/installation-archive-unavailable') throw cause;
        const anchor = await resolveAnchorInstallation(anchorInstallAnchor, name, signal);
        if (!anchor || anchor.manifest.version !== version) throw cause;
        return verifyAnchorInstallation(anchor, isCompatible, signal);
      }
    } : localVerifier;
    const registration=new HostWorkbenchRegistration<Page>(loader,reader,verifier);
    ctx.effect(()=>()=>{registration.close();return archives.close();},'quantdsh.workbench.registration.close');
    const identity = ctx.get('quantdshIdentity') as IdentityService | undefined;
    const connection = ctx.get('connection') as HostConnectionHandle | undefined;
    if (identity && connection) ctx.effect(() => connection.fetch.register({ path: workbenchRegistrationPath, methods: ['POST'], requestBody: 'streaming',
      fetch: request => handleWorkbenchRegistrationRequest(request, registration, async signal => {
        const actor = await identity.resolve(undefined, signal); assertActorContext(actor);
        return () => {
          signal.throwIfAborted(); ctx.fiber.assertActive();
          const profile = identity.profile();
          if (profile.principalId !== actor.principalId || profile.organization.id !== actor.organizationId || profile.resolvedBy !== actor.resolvedBy ||
              identity.membership(actor.organizationId, actor.principalId)?.state !== 'active') throw new Error('workbench/development-access-denied');
        };
      }) }), 'quantdsh.workbench.registration.rpc');
    ctx.provide('quantdshWorkbenchInstallationArchives',archives);
    ctx.provide('quantdshWorkbenchRegistration',registration);
    return Object.freeze({registration,archives,installationId,directory});
  } catch(cause) {await archives.close();throw cause;}
}
