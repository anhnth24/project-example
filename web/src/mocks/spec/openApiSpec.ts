/**
 * Loads `crates/server/openapi/openapi.yaml` (via Vite's `?raw` import — see
 * `yaml.ts` for why we parse it ourselves instead of adding a YAML dependency),
 * resolves internal `$ref`s, and builds a lookup index keyed by `operationId`.
 *
 * This is the runtime source of truth the mock handlers and the spec-drift test
 * are checked against. It reads the *document*, not the generated
 * `src/api/generated/contract.ts` — so it stays authoritative even in the
 * (should-never-happen-but-might) window where contract.ts has drifted from
 * the spec, and it doesn't require `pnpm api:generate` to have been re-run.
 */
import { parseYaml, type YamlValue } from './yaml';
import { specText } from './specSource';

type YamlObject = Record<string, YamlValue>;

function asObject(value: YamlValue | undefined): YamlObject {
  if (value === null || value === undefined || typeof value !== 'object' || Array.isArray(value)) {
    return {};
  }
  return value as YamlObject;
}

function asArray(value: YamlValue | undefined): YamlValue[] {
  return Array.isArray(value) ? value : [];
}

const HTTP_METHODS = ['get', 'put', 'post', 'delete', 'options', 'head', 'patch', 'trace'] as const;
export type HttpMethod = (typeof HTTP_METHODS)[number];

export interface ParameterDef {
  name: string;
  in: 'path' | 'query' | 'header' | 'cookie';
  required: boolean;
  schema: YamlObject;
}

export interface MediaTypeMap {
  [mediaType: string]: YamlObject;
}

export interface ResponseDef {
  status: string;
  description?: string;
  content: MediaTypeMap;
}

export interface RequestBodyDef {
  required: boolean;
  content: MediaTypeMap;
}

export interface OperationDef {
  operationId: string;
  method: HttpMethod;
  path: string;
  parameters: ParameterDef[];
  requestBody?: RequestBodyDef;
  responses: Record<string, ResponseDef>;
  /** Whether this operation requires `Authorization: Bearer <token>` per the spec's security rules. */
  requiresAuth: boolean;
}

export interface SpecIndex {
  raw: YamlObject;
  schemas: Record<string, YamlObject>;
  operations: Record<string, OperationDef>;
  operationIds: string[];
}

function resolveRef(root: YamlObject, node: YamlValue, guard = 0): YamlValue {
  if (guard > 20) throw new Error('spec: $ref cycle guard tripped');
  if (
    node !== null &&
    typeof node === 'object' &&
    !Array.isArray(node) &&
    typeof node.$ref === 'string'
  ) {
    const ref = node.$ref;
    if (!ref.startsWith('#/')) throw new Error(`spec: only local $refs are supported, got ${ref}`);
    const segments = ref.slice(2).split('/');
    let cursor: YamlValue = root;
    for (const seg of segments) {
      cursor = asObject(cursor)[seg] ?? null;
    }
    return resolveRef(root, cursor, guard + 1);
  }
  return node;
}

function resolveObject(root: YamlObject, node: YamlValue): YamlObject {
  return asObject(resolveRef(root, node));
}

function parseParameter(root: YamlObject, node: YamlValue): ParameterDef {
  const resolved = resolveObject(root, node);
  return {
    name: String(resolved.name ?? ''),
    in: (resolved.in as ParameterDef['in']) ?? 'query',
    required: Boolean(resolved.required ?? false),
    schema: asObject(resolved.schema),
  };
}

function parseContent(root: YamlObject, contentNode: YamlValue): MediaTypeMap {
  const content = asObject(contentNode);
  const out: MediaTypeMap = {};
  for (const [mediaType, schemaNode] of Object.entries(content)) {
    out[mediaType] = resolveObject(root, asObject(schemaNode).schema);
  }
  return out;
}

function parseResponses(root: YamlObject, responsesNode: YamlValue): Record<string, ResponseDef> {
  const responses = asObject(responsesNode);
  const out: Record<string, ResponseDef> = {};
  for (const [status, respNode] of Object.entries(responses)) {
    const resolved = resolveObject(root, respNode);
    out[status] = {
      status,
      description: typeof resolved.description === 'string' ? resolved.description : undefined,
      content: parseContent(root, resolved.content),
    };
  }
  return out;
}

export function buildSpecIndex(source: string = specText): SpecIndex {
  const raw = asObject(parseYaml(source));
  const components = asObject(raw.components);
  const schemas: Record<string, YamlObject> = {};
  for (const [name, node] of Object.entries(asObject(components.schemas))) {
    schemas[name] = asObject(node);
  }

  const rootRequiresAuth = asArray(raw.security).length > 0;

  const operations: Record<string, OperationDef> = {};
  const paths = asObject(raw.paths);
  for (const [path, pathItemNode] of Object.entries(paths)) {
    const pathItem = asObject(pathItemNode);
    const sharedParams = asArray(pathItem.parameters).map((p) => parseParameter(raw, p));
    for (const method of HTTP_METHODS) {
      const opNode = pathItem[method];
      if (opNode === undefined || opNode === null) continue;
      const op = asObject(opNode);
      const operationId = String(op.operationId ?? '');
      if (!operationId) continue;
      const ownParams = asArray(op.parameters).map((p) => parseParameter(raw, p));
      let requestBody: RequestBodyDef | undefined;
      if (op.requestBody !== undefined && op.requestBody !== null) {
        const rb = resolveObject(raw, op.requestBody);
        requestBody = {
          required: Boolean(rb.required ?? false),
          content: parseContent(raw, rb.content),
        };
      }
      const requiresAuth = 'security' in op ? asArray(op.security).length > 0 : rootRequiresAuth;

      operations[operationId] = {
        operationId,
        method,
        path,
        parameters: [...sharedParams, ...ownParams],
        requestBody,
        responses: parseResponses(raw, op.responses),
        requiresAuth,
      };
    }
  }

  return { raw, schemas, operations, operationIds: Object.keys(operations) };
}

let cached: SpecIndex | undefined;
/** The parsed+indexed spec, memoized (parsing ~1500 lines once per process is plenty cheap, but no need to repeat it). */
export function getSpecIndex(): SpecIndex {
  if (!cached) cached = buildSpecIndex();
  return cached;
}

export function getOperation(operationId: string): OperationDef {
  const op = getSpecIndex().operations[operationId];
  if (!op) {
    throw new Error(
      `mocks/spec: operationId "${operationId}" is not declared in crates/server/openapi/openapi.yaml. ` +
        `Either the mock is stale (rename/remove the handler) or the spec regressed.`,
    );
  }
  return op;
}

/** Resolves `$ref` for any node against the loaded document; exported for schema validation use. */
export function resolveAgainstSpec(node: YamlValue): YamlValue {
  return resolveRef(getSpecIndex().raw, node);
}
