// The view-spec validator (APS 21 §1). A spec is checked in three steps, the
// same three the agent takes:
//   1. choose  — the spec is well formed, the component exists, its version matches;
//   2. query   — the binding finds the rows (and groups) in the actual ZQL
//                result, and every role reads its type there;
//   3. configure — surface parameters and options conform to the contract.
// Failures are teaching errors in the style of ZQL's (zega#86): what is wrong
// (`message`), where (`at`), a fix in words (`help`) and, where one exists, a
// machine-applicable fix (`fix`: a JSON Patch against the spec). A spec that
// fails never renders.
import SPEC_SCHEMA from './schemas/v1/view-spec.schema.json' with { type: 'json' };
import { check, closest, describe } from './schema.js';
import { migrate } from './migrate.js';

export class ViewSpecError extends Error {
  constructor(errors) {
    super(renderErrors(errors));
    this.name = 'ViewSpecError';
    this.errors = errors;
  }
}

/** The errors as text, one block per error, like a ZQL diagnostic. */
export function renderErrors(errors) {
  return errors.map(({ at, message, help }) => `error: ${message}\n  at: ${at}${help ? `\n  help: ${help}` : ''}`).join('\n\n');
}

/** Apply a `fix` (JSON Patch: add, replace, remove, move) to a copy of `spec`. */
export function applyFix(spec, fix) {
  const out = structuredClone(spec);
  const parts = (pointer) => pointer.split('/').slice(1).map((p) => p.replace(/~1/g, '/').replace(/~0/g, '~'));
  const parent = (keys) => {
    let node = out;
    for (const key of keys.slice(0, -1)) {
      if (!Object.hasOwn(node, key)) node[key] = {};
      node = node[key];
    }
    return node;
  };
  for (const op of fix) {
    const keys = parts(op.path);
    if (op.op === 'add' || op.op === 'replace') parent(keys)[keys.at(-1)] = structuredClone(op.value);
    else if (op.op === 'remove') delete parent(keys)[keys.at(-1)];
    else if (op.op === 'move') {
      const from = parts(op.from);
      const source = parent(from);
      const value = source[from.at(-1)];
      delete source[from.at(-1)];
      parent(keys)[keys.at(-1)] = value;
    } else throw new Error(`Unsupported patch op ${op.op}`);
  }
  return out;
}

const own = (object, key) => object !== null && typeof object === 'object' && !Array.isArray(object) && Object.hasOwn(object, key);
const isObject = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const typeName = (value) => value === null ? 'null' : value === undefined ? 'nothing' : Array.isArray(value) ? 'a list'
  : typeof value === 'number' ? (Number.isInteger(value) ? 'Int' : Number.isFinite(value) ? 'Float' : String(value))
  : typeof value === 'string' ? 'String' : typeof value === 'boolean' ? 'Bool'
  : own(value, 'lat') && own(value, 'lon') ? 'Point' : own(value, 'x') && own(value, 'y') ? 'XY' : 'an object';
const nameOf = (role) => ({ Point: 'a Point', Float: 'a Float', Int: 'an Int', Bool: 'a Bool', String: 'a String', Enum: 'an Enum', XY: 'an XY' })[role.type] || role.type;
const ranged = (v, role) => (role.minimum === undefined || v >= role.minimum) && (role.maximum === undefined || v <= role.maximum);
const range = (role) => `[${role.minimum ?? '-∞'}, ${role.maximum ?? '∞'}]`;

/**
 * Read one value of `role.type`. Returns `{ value }` (normalised: a Point is
 * [lon, lat], an XY is [x, y]) or `{ error }`. `rules.pointLat` narrows a
 * Point's latitude (the map surface: Web Mercator's ±85.0511).
 */
function readOne(value, role, rules) {
  switch (role.type) {
    case 'Point': {
      if (!isObject(value) || !own(value, 'lat') || !own(value, 'lon')) return { error: `is ${typeName(value)}, not a Point` };
      const { lat, lon } = value;
      if (!Number.isFinite(lat) || !Number.isFinite(lon)) return { error: `is a Point with a non-finite coordinate (${lat}, ${lon})` };
      const limit = rules.pointLat ?? 90;
      if (lon < -180 || lon > 180 || lat < -limit || lat > limit) return { error: `is (${lat}, ${lon}), outside lat ±${limit} / lon ±180${limit < 90 ? ' (a Web Mercator map cannot show it)' : ''}` };
      return { value: [lon, lat] };
    }
    case 'Float':
    case 'Int': {
      if (typeof value !== 'number' || !Number.isFinite(value) || (role.type === 'Int' && !Number.isInteger(value))) return { error: `is ${typeName(value)}, not ${nameOf(role)}` };
      if (!ranged(value, role)) return { error: `is ${value}, outside ${range(role)}` };
      return { value };
    }
    case 'Bool': return typeof value === 'boolean' ? { value } : { error: `is ${typeName(value)}, not a Bool` };
    case 'String': return typeof value === 'string' ? { value } : { error: `is ${typeName(value)} (${describe(value)}), not a String` };
    case 'Enum': return typeof value === 'string' && role.enum.includes(value) ? { value } : { error: `is ${describe(value)}, not one of ${role.enum.map((v) => JSON.stringify(v)).join(', ')}` };
    case 'XY': {
      if (!isObject(value) || !Number.isFinite(value.x) || !Number.isFinite(value.y) || !own(value, 'x') || !own(value, 'y')) return { error: `is ${typeName(value)}, not an XY {x, y}` };
      if (!ranged(value.x, role) || !ranged(value.y, role)) return { error: `is (${value.x}, ${value.y}), outside ${range(role)}${role.unit ? ` ${role.unit}` : ''}` };
      return { value: [value.x, value.y] };
    }
    default: return { error: `has unknown role type ${role.type}` };
  }
}
/** Read a role value of `role.shape` (one | list | list<list>). */
function readRole(value, role, rules) {
  const shape = role.shape || 'one';
  if (shape === 'one') return readOne(value, role, rules);
  const depth = shape === 'list' ? 1 : 2;
  const walk = (v, d) => {
    if (d === 0) return readOne(v, role, rules);
    if (!Array.isArray(v)) return { error: `is ${typeName(v)}, not a list` };
    const out = new Array(v.length);
    for (let i = 0; i < v.length; i++) {
      const got = walk(v[i], d - 1);
      if (got.error) return { error: `item ${i} ${got.error}` };
      out[i] = got.value;
    }
    return { value: out };
  };
  return walk(value, depth);
}

/** Read a relative field path (`a.b`, or `.` for the value itself) with own-property steps only. */
function read(value, path) {
  if (path === '.') return { value };
  let node = value;
  const steps = path.split('.');
  for (let i = 0; i < steps.length; i++) {
    const trail = steps.slice(0, i).join('.');
    if (Array.isArray(node)) return { error: `\`${trail}\` is a list`, list: true };
    if (!own(node, steps[i])) return { error: 'missing', fields: isObject(node) ? Object.keys(node).slice(0, 200) : [], trail, step: steps[i] };
    node = node[steps[i]];
  }
  return { value: node };
}

/** Fields of `sample` (one level, and one level into objects) that read as `role`, shallowest first. */
function candidates(sample, role, rules, limit = 3) {
  const found = [];
  const keys = isObject(sample) ? Object.keys(sample).slice(0, 200) : [];
  for (const key of keys) if (found.length < limit && !readRole(sample[key], role, rules).error) found.push(key);
  for (const key of keys) {
    if (found.length >= limit || !isObject(sample[key])) continue;
    for (const inner of Object.keys(sample[key]).slice(0, 50)) {
      if (found.length < limit && !readRole(sample[key][inner], role, rules).error) found.push(`${key}.${inner}`);
    }
  }
  return found;
}
/** Fields of `sample` that hold a list of objects (rows or groups). */
function lists(sample) {
  const found = [];
  if (Array.isArray(sample) && sample.length && isObject(sample[0])) found.push('.');
  if (isObject(sample)) for (const key of Object.keys(sample).slice(0, 200)) if (Array.isArray(sample[key]) && sample[key].length && isObject(sample[key][0])) found.push(key);
  return found.slice(0, 3);
}

const major = (version) => String(version).split('.')[0];
const minor = (version) => Number(String(version).split('.')[1] || 0);
const LEVELS = { result: 'the result', group: 'each group', row: 'each row' };

/**
 * Validate a view spec against its component's contract and the actual ZQL
 * result. Returns `{ ok: true, spec, data, warnings }` — `spec` in the current
 * format with every default filled in, `data` the normalised role values:
 *   { result: { role: value }, groups: [{ role: value, rows: { role: [values] } }] }
 * — or `{ ok: false, errors, warnings }`. `rules` narrows values for the
 * contract's surface (e.g. `{ pointLat: 85.0511 }` on a map).
 */
export function validate(input, result, contract, rules = {}) {
  try {
    return validateInner(input, result, contract, rules);
  } catch (error) {
    return { ok: false, warnings: [], errors: [{ code: 'unreadable', at: 'result', message: `the spec or result could not be read: ${error.message}`, help: 'pass plain JSON (a ZQL result as returned)' }] };
  }
}

function validateInner(input, result, contract, rules) {
  const warnings = [];
  const { spec, from } = migrate(input);
  if (from !== 1 && from === 0) warnings.push({ code: 'migrated', at: 'spec.format', message: 'this spec is format 0; it was migrated to format 1', help: 'save the migrated spec' });
  const errors = check(spec, SPEC_SCHEMA).map((e) => ({ ...e, at: e.at === '(root)' ? 'spec' : e.at }));
  if (errors.length) return { ok: false, errors, warnings };
  if (!contract) return { ok: false, warnings, errors: [{ code: 'component', at: 'component', message: `no component \`${spec.component}\``, help: '' }] };
  if (spec.component !== contract.id) errors.push({ code: 'component', at: 'component', message: `spec is for \`${spec.component}\`, contract is \`${contract.id}\``, help: '' });
  if (major(spec.version) !== major(contract.version)) {
    errors.push({ code: 'version', at: 'version', message: `\`${contract.id}\` is version ${contract.version}; this spec was written for ${spec.version}`,
      help: `re-check the spec against the current contract and set "version": "${major(contract.version)}"`, fix: [{ op: 'replace', path: '/version', value: major(contract.version) }] });
  } else if (minor(spec.version) > minor(contract.version)) {
    warnings.push({ code: 'newer', at: 'version', message: `this spec was written for ${contract.id} ${spec.version}; this runtime has ${contract.version}`, help: 'anything the newer version added is refused below' });
  }

  // 2. query: the binding's structure, then each role, against the actual result.
  const roles = contract.roles;
  const binding = spec.binding;
  const data = { result: Object.create(null), groups: [] };
  for (const key of Object.keys(binding)) {
    if (key === 'rows' || key === 'groups' || own(roles, key)) continue;
    const guess = closest(key, Object.keys(roles));
    errors.push({ code: 'unknown-role', at: `binding.${key}`, message: `\`${contract.id}\` has no role \`${key}\``,
      help: guess ? `did you mean \`${guess}\`?` : `its roles are ${Object.keys(roles).map((r) => `\`${r}\``).join(', ')}`,
      fix: guess && !own(binding, guess) ? [{ op: 'move', from: `/binding/${key}`, path: `/binding/${guess}` }] : [{ op: 'remove', path: `/binding/${key}` }] });
  }
  const bad = (code, at, message, help, fix) => { errors.push({ code, at, message, help, ...(fix ? { fix } : {}) }); };

  // Groups: the result itself (one group), or a list in it.
  let groups = [result];
  if (own(binding, 'groups')) {
    if (contract.data.groups === 'none') bad('groups', 'binding.groups', `\`${contract.id}\` does not take groups`, 'remove `groups`', [{ op: 'remove', path: '/binding/groups' }]);
    else {
      const got = read(result, binding.groups);
      if (got.error || !Array.isArray(got.value) || !got.value.every(isObject)) {
        const found = lists(result);
        bad(got.error === 'missing' ? 'unresolved' : 'type', 'binding.groups', got.error === 'missing' ? `\`${binding.groups}\` is not in the result` : `\`${binding.groups}\` is not a list of objects`,
          found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : 'select a list in the ZQL query', found.length ? [{ op: 'replace', path: '/binding/groups', value: found[0] }] : undefined);
        groups = null;
      } else groups = got.value;
    }
  } else if (contract.data.groups === 'required') {
    const found = lists(result);
    bad('missing-groups', 'binding.groups', `\`${contract.id}\` needs \`groups\`: a list in the result`, found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : 'select a list in the ZQL query',
      found.length ? [{ op: 'add', path: '/binding/groups', value: found[0] }] : undefined);
    groups = null;
  }

  // Rows: a list of objects in each group.
  const perRow = Object.values(roles).some((role) => role.per === 'row');
  let rowsOf = null;
  if (groups && perRow) {
    if (!own(binding, 'rows')) {
      const found = lists(groups[0]);
      bad('missing-rows', 'binding.rows', `\`${contract.id}\` reads its data from rows: bind \`rows\` to a list of them`, found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : 'select a list in the ZQL query',
        found.length ? [{ op: 'add', path: '/binding/rows', value: found[0] }] : undefined);
    } else {
      rowsOf = [];
      for (let g = 0; g < groups.length; g++) {
        const got = read(groups[g], binding.rows);
        const where = groups.length > 1 || own(binding, 'groups') ? ` (group ${g})` : '';
        if (got.error || !Array.isArray(got.value) || !got.value.every(isObject)) {
          const found = lists(groups[g]);
          bad(got.error === 'missing' ? 'unresolved' : 'type', 'binding.rows', got.error === 'missing' ? `\`${binding.rows}\` is not in ${own(binding, 'groups') ? 'each group' : 'the result'}${where}` : `\`${binding.rows}\` is not a list of objects${where}`,
            found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : got.list ? 'a list inside a list: bind the outer one as `groups`' : 'select a list in the ZQL query',
            found.length ? [{ op: 'replace', path: '/binding/rows', value: found[0] }] : undefined);
          rowsOf = null;
          break;
        }
        if (contract.data.minRows !== undefined && got.value.length < contract.data.minRows) {
          bad('length', 'binding.rows', `a path needs at least ${contract.data.minRows} rows; ${where ? `group ${g}` : 'the result'} has ${got.value.length}`, 'widen the query');
        }
        rowsOf.push(got.value);
      }
    }
  } else if (own(binding, 'rows') && !perRow) {
    bad('rows', 'binding.rows', `\`${contract.id}\` has no per-row roles`, 'remove `rows`', [{ op: 'remove', path: '/binding/rows' }]);
  }

  // Roles.
  if (groups) {
    for (const g of groups) data.groups.push({ rows: Object.create(null) });
    for (const [name, role] of Object.entries(roles)) {
      const at = `binding.${name}`;
      const sources = role.per === 'result' ? [result] : role.per === 'group' ? groups : rowsOf;
      if (!sources) continue; // the rows did not resolve; that error is already reported
      const sampleOf = () => (role.per === 'row' ? rowsOf[0]?.[0] : sources[0]);
      const suggest = () => candidates(sampleOf(), role, rules);
      const levelName = role.per === 'row' ? `each row of \`${binding.rows}\`` : role.per === 'group' && !own(binding, 'groups') ? LEVELS.result : LEVELS[role.per];
      if (!own(binding, name)) {
        if (role.required) {
          const found = suggest();
          bad('missing-role', at, `role \`${name}\` is required: ${nameOf(role)} from ${levelName}`,
            found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : `select a field in the ZQL query that holds ${nameOf(role)}`,
            found.length ? [{ op: 'add', path: `/binding/${name}`, value: found[0] }] : undefined);
        }
        continue;
      }
      const path = binding[name];
      const readAll = (items, put) => {
        for (let i = 0; i < items.length; i++) {
          const got = read(items[i], path);
          if (got.error) {
            const fields = got.fields || [];
            const guess = got.error === 'missing' ? closest(got.step, fields) : null;
            const found = guess ? [] : suggest();
            const place = got.trail ? `\`${got.trail}\` of ${levelName}` : levelName;
            bad('unresolved', at, got.error === 'missing' ? `\`${got.step}\` is not in ${place}` : got.error,
              guess ? `did you mean \`${got.trail ? `${got.trail}.` : ''}${guess}\`?` : found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : fields.length ? `it has ${fields.slice(0, 8).map((f) => `\`${f}\``).join(', ')}` : '',
              guess ? [{ op: 'replace', path: `/binding/${name}`, value: `${got.trail ? `${got.trail}.` : ''}${guess}` }] : found.length ? [{ op: 'replace', path: `/binding/${name}`, value: found[0] }] : undefined);
            return false;
          }
          const value = readRole(got.value, role, rules);
          if (value.error) {
            const found = suggest().filter((f) => f !== path);
            const which = items.length > 1 ? ` (${role.per} ${i})` : '';
            bad('type', at, `role \`${name}\` needs ${nameOf(role)}, but \`${path}\`${which} ${value.error}`,
              found.length ? `bind it to ${found.map((f) => `\`${f}\``).join(' or ')}` : `select a field in the ZQL query that holds ${nameOf(role)}`,
              found.length ? [{ op: 'replace', path: `/binding/${name}`, value: found[0] }] : undefined);
            return false;
          }
          put(i, value.value);
        }
        return true;
      };
      if (role.per === 'result') readAll([result], (_, v) => { data.result[name] = v; });
      else if (role.per === 'group') readAll(groups, (i, v) => { data.groups[i][name] = v; });
      else {
        for (let g = 0; g < rowsOf.length; g++) {
          const out = new Array(rowsOf[g].length);
          if (!readAll(rowsOf[g], (i, v) => { out[i] = v; })) break;
          data.groups[g].rows[name] = out;
        }
      }
    }
  }

  // 3. configure: surface parameters and options against the descriptors.
  const settings = (kind, given, descriptors) => {
    const out = {};
    for (const key of Object.keys(given || {})) {
      if (!own(descriptors, key)) {
        const guess = closest(key, Object.keys(descriptors));
        bad('unknown-option', `${kind}.${key}`, `\`${contract.id}\` has no ${kind === 'options' ? 'option' : 'surface parameter'} \`${key}\``,
          guess ? `did you mean \`${guess}\`?` : `it has ${Object.keys(descriptors).map((k) => `\`${k}\``).join(', ') || 'none'}`,
          guess && !own(given, guess) ? [{ op: 'move', from: `/${kind}/${key}`, path: `/${kind}/${guess}` }] : [{ op: 'remove', path: `/${kind}/${key}` }]);
        continue;
      }
      const problems = check(given[key], descriptors[key], `${kind}.${key}`);
      errors.push(...problems.map((p) => ({ ...p, code: p.code === 'range' ? 'range' : 'option' })));
    }
    for (const key of Object.keys(descriptors)) out[key] = own(given, key) ? structuredClone(given[key]) : structuredClone(descriptors[key].default);
    return out;
  };
  const surface = settings('surface', spec.surface, contract.surfaceParams);
  const options = settings('options', spec.options, contract.options);

  if (errors.length) return { ok: false, errors, warnings };
  const cleanBinding = {};
  for (const key of ['groups', 'rows', ...Object.keys(roles)]) if (own(binding, key)) cleanBinding[key] = binding[key];
  return { ok: true, warnings, data,
    spec: { format: 1, component: spec.component, version: spec.version, binding: cleanBinding, surface, options } };
}
