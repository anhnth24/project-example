import { describe, expect, it } from 'vitest';
import { parseYaml } from './yaml';
import { specText } from './specSource';

describe('parseYaml (targeted constructs)', () => {
  it('parses simple block mappings and scalars', () => {
    const doc = parseYaml(
      `title: Hello\nversion: 0.1.0\ncount: 3\nok: true\nnope: false\nblank: null\n`,
    );
    expect(doc).toEqual({
      title: 'Hello',
      version: '0.1.0',
      count: 3,
      ok: true,
      nope: false,
      blank: null,
    });
  });

  it('parses nested block mappings', () => {
    const doc = parseYaml(`a:\n  b: 1\n  c:\n    d: 2\n`);
    expect(doc).toEqual({ a: { b: 1, c: { d: 2 } } });
  });

  it('parses block sequences of plain scalars', () => {
    const doc = parseYaml(`required:\n  - id\n  - name\n  - slug\n`);
    expect(doc).toEqual({ required: ['id', 'name', 'slug'] });
  });

  it('parses "- key: value" continued-mapping sequence items (the parameters[] shape)', () => {
    const doc = parseYaml(
      [
        'parameters:',
        '  - $ref: "#/components/parameters/collectionId"',
        '  - name: limit',
        '    in: query',
        '    schema:',
        '      type: integer',
        '      default: 50',
        '  - name: cursor',
        '    in: query',
      ].join('\n'),
    );
    expect(doc).toEqual({
      parameters: [
        { $ref: '#/components/parameters/collectionId' },
        { name: 'limit', in: 'query', schema: { type: 'integer', default: 50 } },
        { name: 'cursor', in: 'query' },
      ],
    });
  });

  it('parses flow sequences and flow mappings, including quoted "null" as a string', () => {
    const doc = parseYaml(`a: [file, collectionId]\nb: { type: [integer, "null"] }\nc: []\n`);
    expect(doc).toEqual({
      a: ['file', 'collectionId'],
      b: { type: ['integer', 'null'] },
      c: [],
    });
    // The quoted "null" must stay the string "null", not become YAML null — it's a
    // JSON Schema type-name entry (`type: [integer, "null"]`), not an absent value.
    expect((doc as { b: { type: unknown[] } }).b.type[1]).toBe('null');
    expect((doc as { b: { type: unknown[] } }).b.type[1]).not.toBeNull();
  });

  it('parses quoted mapping keys (numeric-looking response status codes)', () => {
    const doc = parseYaml(
      `responses:\n  "200":\n    description: ok\n  "429":\n    description: rate limited\n`,
    );
    expect(doc).toEqual({
      responses: {
        '200': { description: 'ok' },
        '429': { description: 'rate limited' },
      },
    });
  });

  it('parses folded (>) block scalars without choking on colons/slashes inside them', () => {
    const doc = parseYaml(
      [
        'description: >',
        '  Line one has a colon: yes.',
        '  Line two has a slash /like/this.',
        'next: value',
      ].join('\n'),
    );
    expect(doc).toEqual({
      description: 'Line one has a colon: yes. Line two has a slash /like/this.',
      next: 'value',
    });
  });

  it('parses path keys containing literal braces without treating them as flow collections', () => {
    const doc = parseYaml(
      `paths:\n  /collections/{collectionId}:\n    get:\n      operationId: getCollection\n`,
    );
    expect(doc).toEqual({
      paths: {
        '/collections/{collectionId}': {
          get: { operationId: 'getCollection' },
        },
      },
    });
  });

  it('parses single-item flow objects on one line, as used for inline response bodies', () => {
    const doc = parseYaml(`documentId: { type: string, format: uuid }\n`);
    expect(doc).toEqual({ documentId: { type: 'string', format: 'uuid' } });
  });
});

describe('parseYaml against the real openapi.yaml', () => {
  const doc = parseYaml(specText) as Record<string, unknown>;

  it('parses without throwing and yields the expected top-level shape', () => {
    expect(doc.openapi).toBe('3.1.0');
    expect(typeof doc.paths).toBe('object');
    expect(typeof doc.components).toBe('object');
  });

  it('finds all 40 component schemas', () => {
    const schemas = (doc.components as Record<string, unknown>).schemas as Record<string, unknown>;
    expect(Object.keys(schemas)).toHaveLength(40);
  });

  it('parses a representative path item with multiple methods and shared parameters', () => {
    const paths = doc.paths as Record<string, unknown>;
    const collectionItem = paths['/collections/{collectionId}'] as Record<string, unknown>;
    expect(Object.keys(collectionItem).sort()).toEqual(['delete', 'get', 'parameters', 'patch']);
    const params = collectionItem.parameters as unknown[];
    expect(params).toEqual([{ $ref: '#/components/parameters/collectionId' }]);
  });

  it('parses the listDocuments operation query parameters (the "- name: value" continuation shape)', () => {
    const paths = doc.paths as Record<string, unknown>;
    const op = (paths['/collections/{collectionId}/documents'] as Record<string, unknown>)
      .get as Record<string, unknown>;
    expect(op.operationId).toBe('listDocuments');
    const params = op.parameters as Record<string, unknown>[];
    expect(params).toHaveLength(3);
    expect(params[1]).toMatchObject({ name: 'limit', in: 'query' });
    expect(params[2]).toMatchObject({ name: 'cursor', in: 'query' });
  });

  it('parses requestBody + multiple non-2xx responses with $ref and inline schemas', () => {
    const paths = doc.paths as Record<string, unknown>;
    const op = (paths['/uploads'] as Record<string, unknown>).post as Record<string, unknown>;
    expect(op.operationId).toBe('createUpload');
    const responses = op.responses as Record<string, unknown>;
    expect(Object.keys(responses).sort()).toEqual([
      '201',
      '400',
      '403',
      '404',
      '409',
      '413',
      '429',
    ]);
  });

  it('parses operations that take no requestBody at all (publish, reindex)', () => {
    const paths = doc.paths as Record<string, unknown>;
    const publish = (
      paths['/documents/{documentId}/versions/{versionId}/publish'] as Record<string, unknown>
    ).post as Record<string, unknown>;
    expect(publish.requestBody).toBeUndefined();
    const reindex = (paths['/documents/{documentId}/reindex'] as Record<string, unknown>)
      .post as Record<string, unknown>;
    expect(reindex.requestBody).toBeUndefined();
  });
});
