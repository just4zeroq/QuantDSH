import type { MembershipRole } from 'quantdsh-contracts';

/** Member information verified by the configured authentication backend. */
export interface VerifiedMember {
  readonly principalId: string;
  readonly organizationId: string;
  readonly active: boolean;
  readonly role?: MembershipRole;
}
