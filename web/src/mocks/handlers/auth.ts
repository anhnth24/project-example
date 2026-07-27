import { registerOperation } from '../registry';
import { apiError, unauthorized } from '../apiError';
import { authContextForHeader, getStore, mintTokenPair, toMeResponse } from '../fixtures';
import type { components } from '../../api/generated/contract';

type LoginRequest = components['schemas']['LoginRequest'];
type RefreshTokenRequest = components['schemas']['RefreshTokenRequest'];
type TokenResponse = components['schemas']['TokenResponse'];

function tokenResponse(
  userId: string,
  orgId: string,
  pair: { accessToken: string; refreshToken: string },
): TokenResponse {
  return {
    accessToken: pair.accessToken,
    refreshToken: pair.refreshToken,
    tokenType: 'Bearer',
    expiresIn: 3600,
    orgId,
    userId,
  };
}

registerOperation('authLogin', async (ctx) => {
  const body = await ctx.json<LoginRequest>();
  const user = getStore().users.find((u) => u.email === body.email && u.password === body.password);
  if (!user) {
    return { status: 401, body: apiError('unauthorized', 'Email or password is incorrect.') };
  }
  const pair = mintTokenPair(user);
  return { status: 200, body: tokenResponse(user.userId, user.orgId, pair) };
});

registerOperation('authRefresh', async (ctx) => {
  const body = await ctx.json<RefreshTokenRequest>();
  const userId = getStore().refreshTokens.get(body.refreshToken);
  if (!userId) {
    return {
      status: 401,
      body: apiError('unauthorized', 'Refresh token is invalid, expired, or already used.'),
    };
  }
  getStore().refreshTokens.delete(body.refreshToken); // rotation: old refresh token is single-use
  const user = getStore().users.find((u) => u.userId === userId);
  if (!user) {
    return {
      status: 401,
      body: apiError('unauthorized', 'The account for this refresh token no longer exists.'),
    };
  }
  const pair = mintTokenPair(user);
  return { status: 200, body: tokenResponse(user.userId, user.orgId, pair) };
});

registerOperation('authLogout', async (ctx) => {
  const body = await ctx.json<RefreshTokenRequest>();
  getStore().refreshTokens.delete(body.refreshToken); // idempotent: absent token still returns 204
  return { status: 204 };
});

registerOperation('authMe', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized('No active session for this access token.');
  return { status: 200, body: toMeResponse(auth.user, auth.sessionId) };
});
