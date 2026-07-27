import { describe, expect, it } from 'vitest';
import { validateAgainstSchema, assertMatchesSchema } from './validate';
import { buildSpecIndex } from '../spec/openApiSpec';

describe('validateAgainstSchema', () => {
  it('passes a value that satisfies a simple object schema', () => {
    const schema = {
      type: 'object',
      required: ['status', 'requestId'],
      properties: {
        status: { type: 'string', enum: ['ok'] },
        requestId: { type: 'string', format: 'uuid' },
      },
    };
    const errors = validateAgainstSchema(schema, {
      status: 'ok',
      requestId: '11111111-1111-1111-1111-111111111111',
    });
    expect(errors).toEqual([]);
  });

  it('flags a missing required property', () => {
    const schema = { type: 'object', required: ['a', 'b'], properties: {} };
    const errors = validateAgainstSchema(schema, { a: 1 });
    expect(errors).toEqual(['$.b: missing required property']);
  });

  it('flags a bad enum value', () => {
    const schema = { type: 'string', enum: ['ok'] };
    const errors = validateAgainstSchema(schema, 'not-ok');
    expect(errors[0]).toMatch(/is not one of enum/);
  });

  it('flags a malformed uuid', () => {
    const schema = { type: 'string', format: 'uuid' };
    expect(validateAgainstSchema(schema, 'not-a-uuid')[0]).toMatch(/not a valid uuid/);
    expect(validateAgainstSchema(schema, '11111111-1111-1111-1111-111111111111')).toEqual([]);
  });

  it('flags a malformed date-time', () => {
    const schema = { type: 'string', format: 'date-time' };
    expect(validateAgainstSchema(schema, 'not-a-date')[0]).toMatch(/not a valid date-time/);
    expect(validateAgainstSchema(schema, '2024-01-01T00:00:00Z')).toEqual([]);
  });

  it('supports nullable via a type array (OpenAPI 3.1 style)', () => {
    const schema = { type: ['string', 'null'] };
    expect(validateAgainstSchema(schema, null)).toEqual([]);
    expect(validateAgainstSchema(schema, 'hi')).toEqual([]);
    expect(validateAgainstSchema(schema, 3)[0]).toMatch(/expected type/);
  });

  it('validates array items', () => {
    const schema = { type: 'array', items: { type: 'string' } };
    expect(validateAgainstSchema(schema, ['a', 'b'])).toEqual([]);
    expect(validateAgainstSchema(schema, ['a', 1])[0]).toMatch(/\$\[1\]/);
  });

  it('assertMatchesSchema throws with the accumulated errors', () => {
    expect(() => assertMatchesSchema({ type: 'object', required: ['x'] }, {}, 'thing')).toThrow(
      /schema validation failed/,
    );
  });
});

describe('validateAgainstSchema against real spec schemas', () => {
  const spec = buildSpecIndex();

  it('validates a realistic Collection fixture', () => {
    const errors = validateAgainstSchema(spec.schemas.Collection, {
      id: '11111111-1111-1111-1111-111111111111',
      name: 'Handbook',
      slug: 'handbook',
      description: null,
      visibility: 'org',
      createdAt: '2024-01-01T00:00:00.000Z',
    });
    expect(errors).toEqual([]);
  });

  it('catches a Collection fixture missing a required field', () => {
    const errors = validateAgainstSchema(spec.schemas.Collection, {
      id: '11111111-1111-1111-1111-111111111111',
      slug: 'handbook',
      visibility: 'org',
      createdAt: '2024-01-01T00:00:00.000Z',
    });
    expect(errors).toEqual(['$.name: missing required property']);
  });

  it('validates the ApiError envelope shape', () => {
    const errors = validateAgainstSchema(spec.schemas.ApiError, {
      code: 'not_found',
      message: 'Collection not found',
      requestId: '11111111-1111-1111-1111-111111111111',
    });
    expect(errors).toEqual([]);
  });
});
