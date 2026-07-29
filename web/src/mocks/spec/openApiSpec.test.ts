import { describe, expect, it } from 'vitest';
import { buildSpecIndex, getOperation } from './openApiSpec';

const spec = buildSpecIndex();

describe('buildSpecIndex against the real openapi.yaml', () => {
  it('indexes all 40 component schemas', () => {
    expect(Object.keys(spec.schemas)).toHaveLength(40);
    expect(spec.schemas.TokenResponse).toBeDefined();
    expect(spec.schemas.ApiError).toBeDefined();
  });

  it('indexes every operationId with its method and path', () => {
    expect(spec.operations.authLogin).toMatchObject({ method: 'post', path: '/auth/login' });
    expect(spec.operations.listCollections).toMatchObject({ method: 'get', path: '/collections' });
    expect(spec.operations.createCollection).toMatchObject({
      method: 'post',
      path: '/collections',
    });
    expect(spec.operations.getCollection).toMatchObject({
      method: 'get',
      path: '/collections/{collectionId}',
    });
    expect(spec.operationIds).toContain('reindexDocument');
    expect(spec.operationIds).toContain('askStream');
    expect(spec.operationIds).toContain('jobEvents');
  });

  it('resolves $ref-based error responses down to the ApiError schema', () => {
    const op = spec.operations.authLogin;
    expect(Object.keys(op.responses).sort()).toEqual(['200', '401', '429']);
    const err = op.responses['401'].content['application/json'];
    expect(err.required).toEqual(['code', 'message', 'requestId']);
    const rateLimited = op.responses['429'].content['application/json'];
    expect(rateLimited.required).toEqual(['code', 'message', 'requestId']);
  });

  it('resolves the 200 response schema down to TokenResponse required fields', () => {
    const op = spec.operations.authLogin;
    const ok = op.responses['200'].content['application/json'];
    expect(ok.required).toEqual(
      expect.arrayContaining([
        'accessToken',
        'refreshToken',
        'tokenType',
        'expiresIn',
        'orgId',
        'userId',
      ]),
    );
  });

  it('marks requestBody as required for auth/login, refresh and logout', () => {
    expect(spec.operations.authLogin.requestBody?.required).toBe(true);
    expect(spec.operations.authRefresh.requestBody?.required).toBe(true);
    expect(spec.operations.authLogout.requestBody?.required).toBe(true);
  });

  it('leaves requestBody undefined for the two body-less POSTs (publish, reindex)', () => {
    expect(spec.operations.publishDocumentVersion.requestBody).toBeUndefined();
    expect(spec.operations.reindexDocument.requestBody).toBeUndefined();
  });

  it('captures path parameters, including ones declared on the shared pathItem', () => {
    const op = spec.operations.getCollection;
    expect(op.parameters).toEqual([
      {
        name: 'collectionId',
        in: 'path',
        required: true,
        schema: { type: 'string', format: 'uuid' },
      },
    ]);
  });

  it('captures listDocuments query parameters (limit, cursor) plus the shared collectionId path param', () => {
    const op = spec.operations.listDocuments;
    const names = op.parameters.map((p) => `${p.in}:${p.name}`);
    expect(names).toEqual(['path:collectionId', 'query:limit', 'query:cursor']);
  });

  it('captures the multipart/form-data request body for createUpload', () => {
    const op = spec.operations.createUpload;
    expect(op.requestBody?.required).toBe(true);
    expect(Object.keys(op.requestBody?.content ?? {})).toEqual(['multipart/form-data']);
    const schema = op.requestBody?.content['multipart/form-data'];
    expect(schema?.required).toEqual(['file', 'collectionId']);
  });

  it('exposes 204-only responses with no JSON content (publishDocumentVersion)', () => {
    const op = spec.operations.publishDocumentVersion;
    expect(Object.keys(op.responses)).toEqual(['204', '429']);
    expect(op.responses['204'].content).toEqual({});
  });

  it('marks health/auth/openapi.yaml operations as not requiring auth (explicit `security: []`)', () => {
    expect(spec.operations.healthLive.requiresAuth).toBe(false);
    expect(spec.operations.healthReady.requiresAuth).toBe(false);
    expect(spec.operations.authLogin.requiresAuth).toBe(false);
    expect(spec.operations.authRefresh.requiresAuth).toBe(false);
    expect(spec.operations.authLogout.requiresAuth).toBe(false);
    expect(spec.operations.openapiYaml.requiresAuth).toBe(false);
  });

  it('defaults every other operation to requiring auth (inherited root bearerAuth)', () => {
    expect(spec.operations.authMe.requiresAuth).toBe(true);
    expect(spec.operations.listCollections.requiresAuth).toBe(true);
    expect(spec.operations.search.requiresAuth).toBe(true);
    expect(spec.operations.getJob.requiresAuth).toBe(true);
  });

  it('throws a descriptive error for an operationId the spec does not declare', () => {
    expect(() => getOperation('thisOperationDoesNotExist')).toThrow(/is not declared/);
  });
});
