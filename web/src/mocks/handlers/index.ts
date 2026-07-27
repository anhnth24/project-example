/**
 * Importing this module (for side effects) registers every mocked operation.
 * Split by domain for readability; `registry.ts` is the aggregate registry
 * they all write into.
 */
import './health';
import './auth';
import './library';
import './qa';
