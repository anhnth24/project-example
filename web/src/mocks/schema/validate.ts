/**
 * A minimal JSON-Schema-subset validator, sized exactly for what
 * `crates/server/openapi/openapi.yaml` actually uses: `type` (scalar or an
 * array of types, the OpenAPI 3.1 way of expressing nullable), `enum`,
 * `required` + `properties` on objects, `items` on arrays, and `format`
 * (`uuid`, `date-time`; others are accepted without a value-shape check).
 * No `allOf`/`oneOf`/`anyOf`/`pattern`/`$ref` support — the document doesn't
 * use them (verified by inspection; `openApiSpec.ts` resolves `$ref`s before
 * schemas ever reach this validator).
 *
 * This exists so mock fixtures are checked against the spec's shape, not just
 * hand-typed and hoped to match — a required field silently dropped from a
 * fixture is exactly the kind of drift this project can't afford.
 */

export type JsonSchema = Record<string, unknown>;

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function typeOfValue(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'number') return Number.isInteger(value) ? 'integer' : 'number';
  return typeof value;
}

function matchesType(value: unknown, expected: string): boolean {
  const actual = typeOfValue(value);
  if (expected === 'number') return actual === 'number' || actual === 'integer';
  if (expected === 'integer') return actual === 'integer';
  return actual === expected;
}

function checkFormat(format: string, value: unknown, path: string, errors: string[]): void {
  if (typeof value !== 'string') return; // format only meaningfully applies to strings here
  if (format === 'uuid' && !UUID_RE.test(value)) {
    errors.push(`${path}: "${value}" is not a valid uuid`);
  }
  if (format === 'date-time' && Number.isNaN(Date.parse(value))) {
    errors.push(`${path}: "${value}" is not a valid date-time`);
  }
}

export function validateAgainstSchema(
  schema: JsonSchema | undefined,
  value: unknown,
  path = '$',
): string[] {
  if (!schema || Object.keys(schema).length === 0) return []; // untyped / additionalProperties-only schema
  const errors: string[] = [];

  const typeField = schema.type;
  if (typeField !== undefined) {
    const expectedTypes = Array.isArray(typeField) ? (typeField as string[]) : [String(typeField)];
    if (!expectedTypes.some((t) => matchesType(value, t))) {
      errors.push(
        `${path}: expected type ${expectedTypes.join(' | ')}, got ${typeOfValue(value)} (${JSON.stringify(value)})`,
      );
      return errors; // type mismatch makes deeper checks meaningless
    }
  }

  if (Array.isArray(schema.enum) && value !== null && value !== undefined) {
    if (!schema.enum.includes(value as never)) {
      errors.push(
        `${path}: ${JSON.stringify(value)} is not one of enum [${schema.enum.join(', ')}]`,
      );
    }
  }

  if (typeof schema.format === 'string') {
    checkFormat(schema.format, value, path, errors);
  }

  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const obj = value as Record<string, unknown>;
    const required = Array.isArray(schema.required) ? (schema.required as string[]) : [];
    for (const key of required) {
      if (!(key in obj) || obj[key] === undefined) {
        errors.push(`${path}.${key}: missing required property`);
      }
    }
    const properties = (schema.properties as Record<string, JsonSchema> | undefined) ?? {};
    for (const [key, propSchema] of Object.entries(properties)) {
      if (key in obj && obj[key] !== undefined) {
        errors.push(...validateAgainstSchema(propSchema, obj[key], `${path}.${key}`));
      }
    }
  }

  if (Array.isArray(value) && schema.items) {
    value.forEach((item, i) => {
      errors.push(...validateAgainstSchema(schema.items as JsonSchema, item, `${path}[${i}]`));
    });
  }

  return errors;
}

/** Throws with all accumulated errors if the value does not satisfy the schema. */
export function assertMatchesSchema(
  schema: JsonSchema | undefined,
  value: unknown,
  label: string,
): void {
  const errors = validateAgainstSchema(schema, value, label);
  if (errors.length > 0) {
    throw new Error(`schema validation failed:\n  ${errors.join('\n  ')}`);
  }
}
