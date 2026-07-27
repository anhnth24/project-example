/**
 * Minimal YAML-subset parser.
 *
 * This is NOT a general-purpose YAML implementation. It understands exactly the
 * subset of YAML 1.1 that `crates/server/openapi/openapi.yaml` actually uses:
 * block mappings, block sequences (including the `- key: value` continued-mapping
 * form), flow sequences (`[a, b]`), flow mappings (`{a: b, c: d}`), single-line
 * plain/quoted scalars, and folded (`>`)/literal (`|`) block scalars.
 *
 * It deliberately does NOT support: anchors/aliases, multi-document streams,
 * tags, multiline plain scalars, or complex (non-scalar) mapping keys. None of
 * those appear in the OpenAPI document this parser is built for. Confirmed by
 * `grep` against the source file before writing this (see git history / PR
 * description for the audit) — if the document grows one of those constructs,
 * this parser will throw or misparse rather than silently produce wrong data,
 * and the accompanying tests (`yaml.test.ts`, `openApiSpec.test.ts`) exercise it
 * against the real file so drift shows up immediately.
 */

export type YamlValue =
  string | number | boolean | null | YamlValue[] | { [key: string]: YamlValue };

interface Line {
  indent: number;
  text: string; // full original line, including leading whitespace
}

function toLines(source: string): Line[] {
  const rawLines = source.split(/\r\n|\n|\r/);
  const lines: Line[] = [];
  for (const raw of rawLines) {
    if (raw.trim() === '') continue; // blank lines never carry structure here
    if (/^\s*#/.test(raw)) continue; // whole-line comment (not used today, cheap safety net)
    if (raw.trim() === '---' || raw.trim() === '...') continue; // ignore doc markers if present
    const indent = raw.length - raw.trimStart().length;
    lines.push({ indent, text: raw });
  }
  return lines;
}

function parseScalarText(rawText: string): YamlValue {
  const text = rawText.trim();
  if (text === '') return null;
  if (text[0] === '{' || text[0] === '[') {
    const { value } = parseFlow(text, 0);
    return value;
  }
  if (text[0] === '"') return parseDoubleQuoted(text, 0).value;
  if (text[0] === "'") return parseSingleQuoted(text, 0).value;
  if (text === 'true') return true;
  if (text === 'false') return false;
  if (text === 'null' || text === '~') return null;
  if (/^-?\d+$/.test(text)) return Number(text);
  if (/^-?\d+\.\d+$/.test(text)) return Number(text);
  return text;
}

function parseDoubleQuoted(text: string, start: number): { value: string; end: number } {
  // text[start] === '"'
  let i = start + 1;
  let out = '';
  while (i < text.length && text[i] !== '"') {
    if (text[i] === '\\' && i + 1 < text.length) {
      const next = text[i + 1];
      const map: Record<string, string> = { n: '\n', t: '\t', '"': '"', '\\': '\\' };
      out += map[next] ?? next;
      i += 2;
    } else {
      out += text[i];
      i += 1;
    }
  }
  return { value: out, end: i + 1 };
}

function parseSingleQuoted(text: string, start: number): { value: string; end: number } {
  // text[start] === "'"; '' is an escaped single quote in YAML single-quoted scalars.
  let i = start + 1;
  let out = '';
  while (i < text.length) {
    if (text[i] === "'") {
      if (text[i + 1] === "'") {
        out += "'";
        i += 2;
        continue;
      }
      break;
    }
    out += text[i];
    i += 1;
  }
  return { value: out, end: i + 1 };
}

function skipSpaces(text: string, pos: number): number {
  let i = pos;
  while (i < text.length && text[i] === ' ') i += 1;
  return i;
}

function parseFlowScalarToken(text: string, pos: number): { value: YamlValue; end: number } {
  const start = skipSpaces(text, pos);
  if (text[start] === '{' || text[start] === '[') return parseFlow(text, start);
  if (text[start] === '"') return parseDoubleQuoted(text, start);
  if (text[start] === "'") return parseSingleQuoted(text, start);
  let i = start;
  while (i < text.length && text[i] !== ',' && text[i] !== '}' && text[i] !== ']') i += 1;
  const token = text.slice(start, i).trim();
  return { value: parseScalarText(token), end: i };
}

function parseFlow(text: string, pos: number): { value: YamlValue; end: number } {
  const start = skipSpaces(text, pos);
  if (text[start] === '[') {
    let i = skipSpaces(text, start + 1);
    const arr: YamlValue[] = [];
    if (text[i] === ']') return { value: arr, end: i + 1 };
    for (;;) {
      const { value, end } = parseFlowScalarToken(text, i);
      arr.push(value);
      i = skipSpaces(text, end);
      if (text[i] === ',') {
        i = skipSpaces(text, i + 1);
        continue;
      }
      if (text[i] === ']') return { value: arr, end: i + 1 };
      throw new Error(
        `yaml: malformed flow sequence at ${JSON.stringify(text.slice(pos, pos + 40))}`,
      );
    }
  }
  if (text[start] === '{') {
    let i = skipSpaces(text, start + 1);
    const obj: Record<string, YamlValue> = {};
    if (text[i] === '}') return { value: obj, end: i + 1 };
    for (;;) {
      i = skipSpaces(text, i);
      let key: string;
      if (text[i] === '"') {
        const r = parseDoubleQuoted(text, i);
        key = r.value;
        i = r.end;
      } else if (text[i] === "'") {
        const r = parseSingleQuoted(text, i);
        key = r.value;
        i = r.end;
      } else {
        const colon = text.indexOf(':', i);
        key = text.slice(i, colon).trim();
        i = colon;
      }
      i = skipSpaces(text, i);
      if (text[i] !== ':')
        throw new Error(
          `yaml: expected ':' in flow mapping near ${JSON.stringify(text.slice(pos, pos + 40))}`,
        );
      i = skipSpaces(text, i + 1);
      const { value, end } = parseFlowScalarToken(text, i);
      obj[key] = value;
      i = skipSpaces(text, end);
      if (text[i] === ',') {
        i = skipSpaces(text, i + 1);
        continue;
      }
      if (text[i] === '}') return { value: obj, end: i + 1 };
      throw new Error(
        `yaml: malformed flow mapping at ${JSON.stringify(text.slice(pos, pos + 40))}`,
      );
    }
  }
  throw new Error(`yaml: expected '{' or '[' at ${JSON.stringify(text.slice(pos, pos + 40))}`);
}

/** Splits an unquoted-key line's content (after indent) into {key, remainderStartCol}. */
function splitPlainKey(content: string): { key: string; afterKeyOffset: number } {
  // No key in this document contains a colon, so the first top-level ': ' (or a
  // trailing ':' with nothing after it) is always the mapping delimiter.
  const trimmedEnd = content.replace(/\s+$/, '');
  if (trimmedEnd.endsWith(':') && !trimmedEnd.slice(0, -1).includes(': ')) {
    return { key: trimmedEnd.slice(0, -1), afterKeyOffset: trimmedEnd.length };
  }
  const idx = content.indexOf(': ');
  if (idx === -1) {
    throw new Error(`yaml: could not find mapping delimiter in ${JSON.stringify(content)}`);
  }
  return { key: content.slice(0, idx), afterKeyOffset: idx + 2 };
}

/** Parses the key portion of a mapping-entry line (content after indent). */
function parseKey(content: string): { key: string; afterKeyOffset: number } {
  if (content[0] === '"') {
    const r = parseDoubleQuoted(content, 0);
    let i = skipSpaces(content, r.end);
    if (content[i] !== ':')
      throw new Error(`yaml: expected ':' after quoted key in ${JSON.stringify(content)}`);
    i += 1;
    if (content[i] === ' ') i += 1;
    return { key: r.value, afterKeyOffset: i };
  }
  if (content[0] === "'") {
    const r = parseSingleQuoted(content, 0);
    let i = skipSpaces(content, r.end);
    if (content[i] !== ':')
      throw new Error(`yaml: expected ':' after quoted key in ${JSON.stringify(content)}`);
    i += 1;
    if (content[i] === ' ') i += 1;
    return { key: r.value, afterKeyOffset: i };
  }
  return splitPlainKey(content);
}

function isSeqItemLine(line: Line): boolean {
  const rest = line.text.slice(line.indent);
  return rest === '-' || rest.startsWith('- ');
}

/** Consumes a folded (`>`) or literal (`|`) block scalar starting after `lines[idx]`. */
function consumeBlockScalar(
  lines: Line[],
  idx: number,
  parentIndent: number,
  folded: boolean,
): { value: string; idx: number } {
  const chunks: string[] = [];
  let i = idx;
  let blockIndent: number | null = null;
  while (i < lines.length && lines[i].indent > parentIndent) {
    if (blockIndent === null) blockIndent = lines[i].indent;
    chunks.push(lines[i].text.slice(blockIndent));
    i += 1;
  }
  const value = folded ? chunks.join(' ').replace(/\s+/g, ' ').trim() : chunks.join('\n');
  return { value, idx: i };
}

function parseMappingEntry(
  obj: Record<string, YamlValue>,
  lines: Line[],
  idx: number,
  indent: number,
): number {
  const line = lines[idx];
  const content = line.text.slice(indent);
  const { key, afterKeyOffset } = parseKey(content);
  const remainder = content.slice(afterKeyOffset).trim();
  if (remainder === '') {
    const next = lines[idx + 1];
    if (next && next.indent > indent) {
      const nextContent = next.text.slice(next.indent);
      if (nextContent[0] === '[' || nextContent[0] === '{') {
        // A flow collection wrapped onto its own (more-indented) continuation
        // line, e.g. `required:\n  [a, b, c]`. This is a single-line flow value,
        // not the start of a nested block — consume exactly that one line.
        obj[key] = parseScalarText(nextContent);
        return idx + 2;
      }
      const { value, idx: nextIdx } = parseBlockValue(lines, idx + 1, next.indent);
      obj[key] = value;
      return nextIdx;
    }
    obj[key] = null;
    return idx + 1;
  }
  if (remainder === '>' || remainder === '|' || remainder === '>-' || remainder === '|-') {
    const { value, idx: nextIdx } = consumeBlockScalar(
      lines,
      idx + 1,
      indent,
      remainder[0] === '>',
    );
    obj[key] = value;
    return nextIdx;
  }
  obj[key] = parseScalarText(remainder);
  return idx + 1;
}

function parseSequence(
  lines: Line[],
  idx: number,
  indent: number,
): { value: YamlValue[]; idx: number } {
  const items: YamlValue[] = [];
  let i = idx;
  while (i < lines.length && lines[i].indent === indent && isSeqItemLine(lines[i])) {
    const line = lines[i];
    const rest = line.text.slice(indent);
    if (rest === '-') {
      const next = lines[i + 1];
      if (next && next.indent > indent) {
        const { value, idx: nextIdx } = parseBlockValue(lines, i + 1, next.indent);
        items.push(value);
        i = nextIdx;
      } else {
        items.push(null);
        i += 1;
      }
      continue;
    }
    const itemContent = rest.slice(2); // after "- "
    const itemCol = indent + 2;
    // Quoted-key mapping entries (`- "foo": bar`) don't occur in this document,
    // so a quoted first character always means a plain scalar item.
    const looksLikeMapping =
      itemContent[0] !== '"' &&
      itemContent[0] !== "'" &&
      (() => {
        const trimmedEnd = itemContent.replace(/\s+$/, '');
        return (
          (trimmedEnd.endsWith(':') && !trimmedEnd.slice(0, -1).includes(': ')) ||
          itemContent.includes(': ')
        );
      })();
    if (looksLikeMapping) {
      const obj: Record<string, YamlValue> = {};
      const virtualLine: Line = { indent: itemCol, text: ' '.repeat(itemCol) + itemContent };
      const linesForEntry = [virtualLine, ...lines.slice(i + 1)];
      const localIdx = parseMappingEntry(obj, linesForEntry, 0, itemCol);
      // localIdx is relative to linesForEntry; convert continuation scanning to the
      // real array by continuing the standard mapping loop against `lines`.
      let realIdx = i + 1 + (localIdx - 1);
      while (
        realIdx < lines.length &&
        lines[realIdx].indent === itemCol &&
        !isSeqItemLine(lines[realIdx])
      ) {
        realIdx = parseMappingEntry(obj, lines, realIdx, itemCol);
      }
      items.push(obj);
      i = realIdx;
    } else {
      items.push(parseScalarText(itemContent));
      i += 1;
    }
  }
  return { value: items, idx: i };
}

function parseMapping(
  lines: Line[],
  idx: number,
  indent: number,
): { value: Record<string, YamlValue>; idx: number } {
  const obj: Record<string, YamlValue> = {};
  let i = idx;
  while (i < lines.length && lines[i].indent === indent && !isSeqItemLine(lines[i])) {
    i = parseMappingEntry(obj, lines, i, indent);
  }
  return { value: obj, idx: i };
}

function parseBlockValue(
  lines: Line[],
  idx: number,
  indent: number,
): { value: YamlValue; idx: number } {
  if (idx >= lines.length) return { value: null, idx };
  if (isSeqItemLine(lines[idx])) return parseSequence(lines, idx, indent);
  return parseMapping(lines, idx, indent);
}

export function parseYaml(source: string): YamlValue {
  const lines = toLines(source);
  if (lines.length === 0) return null;
  const { value } = parseBlockValue(lines, 0, lines[0].indent);
  return value;
}
