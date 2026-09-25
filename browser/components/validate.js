// The view-spec validator (APS 21 §1). A spec is checked in three steps, the
// same three the agent takes:
//   1. choose  — the component exists and the spec's version matches its contract;
//   2. query   — every binding resolves in the actual ZQL result, to the role's type;
//   3. configure — surface parameters and options conform to the contract.
// Failures are teaching errors in the style of ZQL's (zega#86): what is wrong,
// where, and a `help:` line naming a fix drawn from the result itself. A spec
// that fails never renders.
import SPEC_SCHEMA from './view-spec.schema.json' with { type: 'json' };
import { check, closest, describe } from './schema.js';

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

const typeName = (value) => value === null ? 'null' : Array.isArray(value) ? 'a list'
  : typeof value === 'number' ? (Number.isInteger(value) ? 'Int' : 'Float')
  : typeof value === 'string' ? 'String' : typeof value === 'boolean' ? 'Bool'
  : isPoint(value) ? 'Point' : 'an object';
const isPoint = (v) => v !== null && typeof v === 'object' && !Array.isArray(v) && Number.isFinite(v.lat) && Number.isFinite(v.lon);
const isPair = (v) => Array.isArray(v) && v.length === 2 && v.every(Number.isFinite);
const inRange = ([lon, lat]) => lon >= -180 && lon <= 180 && lat >= -90 && lat <= 90;

/** The role types. Each normalises a resolved value, or explains why it cannot. */
export const ROLE_TYPES = {
  'lonlat[]': {
    label: 'a list of Point (or [lon, lat])',
    read(value) {
      if (!Array.isArray(value)) return { error: `holds ${typeName(value)}` };
      const out = new Array(value.length);
      for (let i = 0; i < value.length; i++) {
        const item = value[i];
        const pair = isPoint(item) ? [item.lon, item.lat] : isPair(item) ? item : null;
        if (!pair) return { error: `item ${i} is ${typeName(item)}${item && typeof item === 'object' && !Array.isArray(item) ? ` with fields ${Object.keys(item).join(', ')}` : ''}, not a Point` };
        if (!inRange(pair)) return { error: `item ${i} is [${pair}], outside lon [-180, 180] / lat [-90, 90]`, help: 'a pair is [lon, lat]; check the order' };
        out[i] = pair;
      }
      return { value: out };
    },
  },
  'f64[]': {
    label: 'a list of numbers',
    read(value) {
      if (!Array.isArray(value)) return { error: `holds ${typeName(value)}` };
      for (let i = 0; i < value.length; i++) {
        if (!Number.isFinite(value[i])) return { error: `item ${i} is ${typeName(value[i])}, not a number` };
      }
      return { value: value.slice() };
    },
  },
  f64: {
    label: 'a number',
    read: (value) => Number.isFinite(value) ? { value } : { error: `holds ${typeName(value)}` },
  },
  string: {
    label: 'a String',
    read: (value) => typeof value === 'string' ? { value } : { error: `holds ${typeName(value)} (${describe(value)})` },
  },
};

/** Parse `points[].at` into steps. */
function steps(path) {
  const out = [];
  for (const part of path.split('.')) {
    if (part === '[]') { out.push({ field: null, each: true }); continue; }
    const each = part.endsWith('[]');
    out.push({ field: each ? part.slice(0, -2) : part, each });
  }
  return out;
}
const fieldsOf = (value) => value && typeof value === 'object' && !Array.isArray(value) ? Object.keys(value) : [];

/**
 * Resolve a binding path in a result. Returns `{ value }` or
 * `{ error, help }` explaining the first step that did not resolve.
 */
export function resolve(result, path) {
  function walk(value, rest, trail) {
    if (!rest.length) return { value };
    const [step, ...more] = rest;
    let next = value;
    if (step.field !== null) {
      if (Array.isArray(value)) {
        return { error: `\`${trail || 'the result'}\` is a list`, help: `write \`${trail ? `${trail}[]` : '[]'}.${step.field}\` to read \`${step.field}\` from each item` };
      }
      if (!value || typeof value !== 'object' || !(step.field in value)) {
        const fields = fieldsOf(value);
        const guess = closest(step.field, fields);
        return {
          error: `\`${step.field}\` is not in ${trail ? `\`${trail}\`` : 'the result'}`,
          help: guess ? `did you mean \`${trail ? `${trail}.` : ''}${guess}\`?` : fields.length ? `it has ${fields.map((f) => `\`${f}\``).join(', ')}; select the field in the ZQL query` : `${trail || 'the result'} is ${typeName(value)}`,
        };
      }
      next = value[step.field];
    }
    const here = step.field === null ? `${trail}[]` : `${trail ? `${trail}.` : ''}${step.field}${step.each ? '[]' : ''}`;
    if (!step.each) return walk(next, more, here);
    if (!Array.isArray(next)) return { error: `\`${here.slice(0, -2) || 'the result'}\` is ${typeName(next)}, not a list`, help: `drop the \`[]\`` };
    const out = new Array(next.length);
    for (let i = 0; i < next.length; i++) {
      const got = walk(next[i], more, here);
      if (got.error) return { ...got, error: `${got.error} (item ${i})` };
      out[i] = got.value;
    }
    return { value: out };
  }
  return walk(result, steps(path), '');
}

/** Paths in the result that would satisfy a role type, shallowest first, for `help:` lines. */
export function candidates(result, type, limit = 3) {
  const found = [];
  const reader = ROLE_TYPES[type];
  let level = [{ value: result, path: '' }];
  for (let depth = 0; depth < 5 && level.length && found.length < limit; depth++) {
    const next = [];
    for (const { value, path } of level) {
      if (path) {
        const got = resolve(result, path);
        if (!got.error && !reader.read(got.value).error) { found.push(path); continue; }
      }
      if (Array.isArray(value)) { if (value.length) next.push({ value: value[0], path: path ? `${path}[]` : '[]' }); continue; }
      for (const key of fieldsOf(value)) next.push({ value: value[key], path: path ? `${path}.${key}` : key });
    }
    level = next;
  }
  return found.slice(0, limit);
}

const major = (version) => String(version).split('.')[0];

/**
 * Validate a view spec against its component's contract and the actual ZQL
 * result. Returns `{ ok: true, spec, data }` — `spec` with every default filled
 * in, `data` the normalised role values — or `{ ok: false, errors }`.
 */
export function validate(spec, result, contract) {
  const errors = check(spec, SPEC_SCHEMA).map((e) => ({ ...e, at: `spec.${e.at}`.replace('spec.(root)', 'spec') }));
  if (errors.length) return { ok: false, errors };
  if (!contract) return { ok: false, errors: [{ code: 'component', at: 'spec.component', message: `no component \`${spec.component}\``, help: '' }] };
  if (spec.component !== contract.id) errors.push({ code: 'component', at: 'spec.component', message: `spec is for \`${spec.component}\`, contract is \`${contract.id}\``, help: '' });
  if (major(spec.version) !== major(contract.version)) {
    errors.push({ code: 'version', at: 'spec.version', message: `\`${contract.id}\` is version ${contract.version}; this spec was written for ${spec.version}`,
      help: `re-check the spec against the current contract and set "version": "${major(contract.version)}"` });
  }

  // 2. query: bindings against the result.
  const data = {};
  const roles = contract.roles;
  for (const [role, path] of Object.entries(spec.binding)) {
    if (!(role in roles)) {
      const guess = closest(role, Object.keys(roles));
      errors.push({ code: 'unknown-role', at: `binding.${role}`, message: `\`${contract.id}\` has no role \`${role}\``,
        help: guess ? `did you mean \`${guess}\`?` : `its roles are ${Object.keys(roles).map((r) => `\`${r}\``).join(', ')}` });
    }
  }
  for (const [role, descriptor] of Object.entries(roles)) {
    const path = spec.binding[role];
    const type = ROLE_TYPES[descriptor.type];
    const suggest = () => {
      const found = candidates(result, descriptor.type);
      return found.length ? `bind it to ${found.map((p) => `\`${p}\``).join(' or ')}` : `select a field in the ZQL query that holds ${type.label}`;
    };
    if (path === undefined) {
      if (descriptor.required) errors.push({ code: 'missing-role', at: `binding.${role}`, message: `role \`${role}\` is required: ${type.label}${descriptor.ordered ? ', in order' : ''}`, help: suggest() });
      continue;
    }
    const got = resolve(result, path);
    if (got.error) { errors.push({ code: 'unresolved', at: `binding.${role} = "${path}"`, message: got.error, help: got.help }); continue; }
    const read = type.read(got.value);
    if (read.error) {
      errors.push({ code: 'type', at: `binding.${role} = "${path}"`, message: `role \`${role}\` needs ${type.label}, but \`${path}\` ${read.error.startsWith('item') ? `has ${read.error}` : read.error}`, help: read.help || suggest() });
      continue;
    }
    if (descriptor.minItems !== undefined && read.value.length < descriptor.minItems) {
      errors.push({ code: 'length', at: `binding.${role} = "${path}"`, message: `role \`${role}\` needs at least ${descriptor.minItems} items; the result has ${read.value.length}`, help: 'widen the query' });
      continue;
    }
    data[role] = read.value;
  }
  for (const [role, descriptor] of Object.entries(roles)) {
    const other = descriptor.sameLengthAs;
    if (other && data[role] && data[other] && data[role].length !== data[other].length) {
      errors.push({ code: 'length', at: `binding.${role} = "${spec.binding[role]}"`, message: `role \`${role}\` has ${data[role].length} items but \`${other}\` has ${data[other].length}; they pair item for item`,
        help: `read both from the same list, e.g. \`${spec.binding[other].replace(/[^.]+$/, '…')}\`` });
    }
  }

  // 3. configure: surface parameters and options against the descriptors.
  const settings = (kind, given, descriptors) => {
    const out = {};
    for (const [key, value] of Object.entries(given || {})) {
      if (!(key in descriptors)) {
        const guess = closest(key, Object.keys(descriptors));
        errors.push({ code: 'unknown-option', at: `${kind}.${key}`, message: `\`${contract.id}\` has no ${kind === 'options' ? 'option' : 'surface parameter'} \`${key}\``,
          help: guess ? `did you mean \`${guess}\`?` : `it has ${Object.keys(descriptors).map((k) => `\`${k}\``).join(', ') || 'none'}` });
        continue;
      }
      const problems = check(value, descriptors[key], `${kind}.${key}`);
      errors.push(...problems.map((p) => ({ ...p, code: p.code === 'range' ? 'range' : 'option' })));
    }
    for (const [key, descriptor] of Object.entries(descriptors)) out[key] = key in (given || {}) ? given[key] : structuredClone(descriptor.default);
    return out;
  };
  const surface = settings('surface', spec.surface, contract.surfaceParams);
  const options = settings('options', spec.options, contract.options);

  if (errors.length) return { ok: false, errors };
  return { ok: true, spec: { component: spec.component, version: spec.version, binding: { ...spec.binding }, surface, options }, data };
}
