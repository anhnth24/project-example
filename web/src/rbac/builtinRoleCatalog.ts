import catalog from '../../../crates/server/openapi/builtin-role-catalog.json';
import type { MembershipRole } from '../components/admin/types';

export const BUILTIN_ROLE_CATALOG = catalog;
export const ROLE_ORDER = catalog.roles as readonly MembershipRole[];
