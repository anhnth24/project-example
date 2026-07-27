/**
 * Single point of contact with the on-disk OpenAPI document. Every other
 * module that needs the spec's raw text or parsed/indexed form imports it
 * from here (or from `openApiSpec.ts`, which itself imports this) rather than
 * re-stating the relative path to `crates/server/openapi/openapi.yaml` — one
 * place to fix if the file ever moves.
 */
import specText from '../../../../crates/server/openapi/openapi.yaml?raw';

export { specText };
