// A small JSON Schema (2020-12) interpreter: the subset the graph-component
// files use. One interpreter checks the view-spec shape
// (view-spec.schema.json), a contract's shape (contract.schema.json), and a
// spec's surface parameters and options against the contract's own
// descriptors, so the published schemas and the runtime cannot disagree.
//
// Supported: type (string or list), enum, const, minimum, maximum, minLength,
// pattern, items, minItems, maxItems, properties, required,
// additionalProperties (false or a schema), $ref to "#/$defs/…", oneOf.
// Every other keyword (title, description, default, unit, …) is annotation.

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

// Edit distance, for "did you mean".
export function closest(word, candidates) {
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

/**
 * Check `value` against `schema`. Returns a list of `{ code, at, message, help }`
 * problems (empty when it conforms). `at` is a dotted path from `where`.
 */
export function check(value, schema, where = '', root = schema) {
  const errors = [];
  const push = (code, message, help = '') => errors.push({ code, at: where || '(root)', message, help });
  if (schema.$ref) {
    const target = schema.$ref.replace(/^#\//, '').split('/').reduce((node, key) => node?.[key], root);
    if (!target) throw new Error(`Unresolved $ref ${schema.$ref}`);
    return check(value, target, where, root);
  }
  if (schema.oneOf) {
    const passing = schema.oneOf.filter((option) => check(value, option, where, root).length === 0);
    if (passing.length !== 1) push('shape', `${describe(value)} matches ${passing.length ? 'more than one' : 'none'} of the allowed forms`, schema.description || '');
    return errors;
  }
  if (schema.type) {
    const types = [schema.type].flat();
    if (!types.some((type) => matchesType(value, type))) {
      push('type', `expected ${types.join(' or ')}, got ${typeOf(value) === 'integer' ? 'number' : typeOf(value)} ${describe(value)}`,
        schema.description || '');
      return errors;
    }
  }
  if ('const' in schema && value !== schema.const) push('const', `must be ${JSON.stringify(schema.const)}, got ${describe(value)}`);
  if (schema.enum && !schema.enum.includes(value)) {
    const guess = typeof value === 'string' ? closest(value, schema.enum.filter((v) => typeof v === 'string')) : null;
    push('enum', `${describe(value)} is not one of ${list(schema.enum)}`, guess ? `did you mean ${JSON.stringify(guess)}?` : `use one of ${list(schema.enum)}`);
  }
  if (typeof value === 'number') {
    if (schema.minimum !== undefined && value < schema.minimum) push('range', `${value} is below the minimum ${schema.minimum}`, `use a value in [${schema.minimum}, ${schema.maximum ?? '∞'}]`);
    if (schema.maximum !== undefined && value > schema.maximum) push('range', `${value} is above the maximum ${schema.maximum}`, `use a value in [${schema.minimum ?? '-∞'}, ${schema.maximum}]`);
  }
  if (typeof value === 'string') {
    if (schema.minLength !== undefined && value.length < schema.minLength) push('length', `must be at least ${schema.minLength} characters`);
    if (schema.pattern && !new RegExp(schema.pattern, 'u').test(value)) push('pattern', `${describe(value)} is not well formed`, schema.description || `it must match ${schema.pattern}`);
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) push('length', `needs at least ${schema.minItems} items, got ${value.length}`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) push('length', `takes at most ${schema.maxItems} items, got ${value.length}`);
    if (schema.items) value.forEach((item, i) => errors.push(...check(item, schema.items, `${where}[${i}]`, root)));
  }
  if (typeOf(value) === 'object') {
    for (const key of schema.required || []) {
      if (!(key in value)) push('required', `\`${key}\` is required`, schema.properties?.[key]?.description || '');
    }
    const known = Object.keys(schema.properties || {});
    for (const [key, item] of Object.entries(value)) {
      const at = where ? `${where}.${key}` : key;
      if (schema.properties && key in schema.properties) errors.push(...check(item, schema.properties[key], at, root));
      else if (schema.additionalProperties === false) {
        const guess = closest(key, known);
        errors.push({ code: 'unknown', at, message: `\`${key}\` is not a known field here`,
          help: guess ? `did you mean \`${guess}\`?` : known.length ? `known fields: ${known.map((k) => `\`${k}\``).join(', ')}` : 'remove it' });
      } else if (typeof schema.additionalProperties === 'object') errors.push(...check(item, schema.additionalProperties, at, root));
    }
  }
  return errors;
}
