// A small JSON Schema (2020-12) interpreter: the subset the graph-component
// files use. One interpreter checks the view-spec shape, a contract's shape,
// and a spec's surface parameters and options against the contract's own
// setting descriptors, so the published schemas and the runtime cannot
// disagree about what a schema means.
//
// Assertion keywords it interprets (SETTING_KEYWORDS is the subset a
// contract's settings may use; contract.schema.json closes `setting` to it):
//   type, enum, const, minimum, maximum, minLength, maxLength, pattern,
//   items, prefixItems, minItems, maxItems, format ("lonlat-bounds"),
//   and for the files' own schemas also properties, required,
//   additionalProperties, $ref ("#/$defs/…"), oneOf.
// Annotation keywords (title, description, default, $schema, $id) are ignored.
//
// Keys are only ever looked up as own properties (Object.hasOwn), so a key
// named `constructor`, `toString` or `__proto__` is unknown, never inherited.

export const SETTING_KEYWORDS = ['default', 'description', 'type', 'enum', 'const', 'minimum', 'maximum', 'minLength', 'maxLength',
  'pattern', 'items', 'prefixItems', 'minItems', 'maxItems', 'format'];

const own = (object, key) => object !== null && typeof object === 'object' && Object.hasOwn(object, key);
const typeOf = (value) => value === null ? 'null' : Array.isArray(value) ? 'array'
  : Number.isInteger(value) ? 'integer' : typeof value;
const matchesType = (value, type) => type === typeOf(value) || (type === 'number' && typeof value === 'number' && Number.isFinite(value));
export const describe = (value) => {
  const kind = typeOf(value);
  if (kind === 'string') return `"${value.length > 40 ? `${value.slice(0, 37)}…` : value}"`;
  if (kind === 'array') return `a list of ${value.length}`;
  if (kind === 'object') return 'an object';
  return String(value);
};
const list = (values) => values.map((v) => JSON.stringify(v)).join(', ');

// Edit distance, for "did you mean". Words over 64 characters are not compared.
export function closest(word, candidates) {
  if (typeof word !== 'string' || word.length > 64) return null;
  let best = null, score = Infinity;
  for (const candidate of candidates) {
    const a = word.toLowerCase(), b = candidate.toLowerCase();
    const row = Array.from({ length: b.length + 1 }, (_, j) => j);
    for (let i = 1; i <= a.length; i++) {
      let prev = row[0];
      row[0] = i;
      for (let j = 1; j <= b.length; j++) {
        const tmp = row[j];
        row[j] = Math.min(row[j] + 1, row[j - 1] + 1, prev + (a[i - 1] === b[j - 1] ? 0 : 1));
        prev = tmp;
      }
    }
    if (row[b.length] < score) { score = row[b.length]; best = candidate; }
  }
  return score <= Math.max(2, Math.floor(word.length / 3)) ? best : null;
}

/** A JSON Pointer from path segments, each escaped (`~` → `~0`, `/` → `~1`). */
export const pointer = (...segments) => segments.map((p) => `/${String(p).replace(/~/g, '~0').replace(/\//g, '~1')}`).join('');
/**
 * A fix: a JSON Patch plus how far it can be trusted. `safe` fixes only
 * repair what the spec already means (a spelling, a clamp into range, an
 * unknown key removed); `guess` fixes choose something (a field that has the
 * right type, a default). Only `safe` fixes may be applied without a person
 * or the planner deciding.
 */
export const fixOf = (kind, patch) => ({ kind, patch });

/**
 * Check `value` against `schema`. Returns a list of `{ code, at, message, help, fix? }`
 * problems (empty when it conforms). `at` is a dotted path from `where`.
 */
export function check(value, schema, where = '', root = schema, depth = 0, segments = where ? where.split('.') : []) {
  const errors = [];
  const here = pointer(...segments);
  const at = where || '(root)';
  const push = (code, message, help = '', fix) => errors.push({ code, at, message, help, ...(fix ? { fix } : {}) });
  if (depth > 64) { push('depth', 'is nested too deeply'); return errors; }
  if (own(schema, '$ref')) {
    const target = schema.$ref.replace(/^#\//, '').split('/').reduce((node, key) => (own(node, key) ? node[key] : undefined), root);
    if (!target) throw new Error(`Unresolved $ref ${schema.$ref}`);
    return check(value, target, where, root, depth + 1, segments);
  }
  if (own(schema, 'oneOf')) {
    const passing = schema.oneOf.filter((option) => check(value, option, where, root, depth + 1, segments).length === 0);
    if (passing.length !== 1) push('shape', `${describe(value)} matches ${passing.length ? 'more than one' : 'none'} of the allowed forms`, schema.description || '');
    return errors;
  }
  if (typeof value === 'number' && !Number.isFinite(value)) { push('type', `${value} is not a finite number`); return errors; }
  if (own(schema, 'type')) {
    const types = [schema.type].flat();
    if (!types.some((type) => matchesType(value, type))) {
      push('type', `expected ${types.join(' or ')}, got ${typeOf(value) === 'integer' ? 'number' : typeOf(value)} ${describe(value)}`,
        schema.description || '', own(schema, 'default') && where ? fixOf('guess', [{ op: 'replace', path: here, value: schema.default }]) : undefined);
      return errors;
    }
  }
  if (own(schema, 'const') && value !== schema.const) push('const', `must be ${JSON.stringify(schema.const)}, got ${describe(value)}`, '', where ? fixOf('safe', [{ op: 'replace', path: here, value: schema.const }]) : undefined);
  if (own(schema, 'enum') && !schema.enum.includes(value)) {
    const guess = typeof value === 'string' ? closest(value, schema.enum.filter((v) => typeof v === 'string')) : null;
    const to = guess ?? (own(schema, 'default') ? schema.default : undefined);
    push('enum', `${describe(value)} is not one of ${list(schema.enum)}`, guess ? `did you mean ${JSON.stringify(guess)}?` : `use one of ${list(schema.enum)}`,
      to !== undefined && where ? fixOf(guess ? 'safe' : 'guess', [{ op: 'replace', path: here, value: to }]) : undefined);
  }
  if (typeof value === 'number') {
    const lo = own(schema, 'minimum') ? schema.minimum : undefined, hi = own(schema, 'maximum') ? schema.maximum : undefined;
    const fix = (to) => (where ? fixOf('safe', [{ op: 'replace', path: here, value: to }]) : undefined);
    if (lo !== undefined && value < lo) push('range', `${value} is below the minimum ${lo}`, `use a value in [${lo}, ${hi ?? '∞'}]`, fix(lo));
    if (hi !== undefined && value > hi) push('range', `${value} is above the maximum ${hi}`, `use a value in [${lo ?? '-∞'}, ${hi}]`, fix(hi));
  }
  if (typeof value === 'string') {
    if (own(schema, 'maxLength') && value.length > schema.maxLength) { push('length', `is longer than ${schema.maxLength} characters`); return errors; }
    if (own(schema, 'minLength') && value.length < schema.minLength) push('length', `must be at least ${schema.minLength} characters`);
    if (own(schema, 'pattern') && !new RegExp(schema.pattern, 'u').test(value)) push('pattern', `${describe(value)} is not well formed`, schema.description || `it must match ${schema.pattern}`);
  }
  if (Array.isArray(value)) {
    if (own(schema, 'minItems') && value.length < schema.minItems) push('length', `needs at least ${schema.minItems} items, got ${value.length}`);
    if (own(schema, 'maxItems') && value.length > schema.maxItems) push('length', `takes at most ${schema.maxItems} items, got ${value.length}`);
    const prefix = own(schema, 'prefixItems') ? schema.prefixItems : [];
    value.forEach((item, i) => {
      const itemSchema = i < prefix.length ? prefix[i] : own(schema, 'items') ? schema.items : null;
      if (itemSchema) errors.push(...check(item, itemSchema, `${where}[${i}]`, root, depth + 1, [...segments, i]));
    });
    if (own(schema, 'format') && schema.format === 'lonlat-bounds' && !errors.length && value.length === 4) {
      const [w, s, e, n] = value;
      if (!(w < e)) push('bounds', `west (${w}) must be less than east (${e})`, 'bounds are [west, south, east, north]');
      if (!(s < n)) push('bounds', `south (${s}) must be less than north (${n})`, 'bounds are [west, south, east, north]');
    }
  }
  if (typeOf(value) === 'object') {
    const properties = own(schema, 'properties') ? schema.properties : {};
    for (const key of own(schema, 'required') ? schema.required : []) {
      if (!own(value, key)) push('required', `\`${key}\` is required`, own(properties, key) ? properties[key].description || '' : '');
    }
    const known = Object.keys(properties);
    for (const key of Object.keys(value)) {
      const path = where ? `${where}.${key}` : key;
      if (own(properties, key)) errors.push(...check(value[key], properties[key], path, root, depth + 1, [...segments, key]));
      else if (own(schema, 'additionalProperties') && schema.additionalProperties === false) {
        const guess = closest(key, known);
        errors.push({ code: 'unknown', at: path, message: `\`${key}\` is not a known field here`,
          help: guess ? `did you mean \`${guess}\`?` : known.length ? `known fields: ${known.map((k) => `\`${k}\``).join(', ')}` : 'remove it',
          fix: guess ? fixOf('safe', [{ op: 'move', from: pointer(...segments, key), path: pointer(...segments, guess) }]) : fixOf('safe', [{ op: 'remove', path: pointer(...segments, key) }]) });
      } else if (own(schema, 'additionalProperties') && typeof schema.additionalProperties === 'object') errors.push(...check(value[key], schema.additionalProperties, path, root, depth + 1, [...segments, key]));
    }
  }
  return errors;
}
