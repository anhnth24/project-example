// Shared type aliases for the admin members/usage UI (P2-11/P2-12). Every
// shape here is derived straight from `api/generated/contract.ts` — nothing
// re-declares a schema that file already exports, same convention as
// `components/library/types.ts`.
import type { components } from '../../api/generated/contract';

export type Membership = components['schemas']['Membership'];
export type MembershipRole = Membership['role'];
export type MembershipState = Membership['state'];
export type Invite = components['schemas']['Invite'];
export type InviteStatus = Invite['status'];
export type UsageEntry = components['schemas']['UsageEntry'];
export type UsageResource = UsageEntry['resource'];
