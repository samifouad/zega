// The graph-component registry: every component's contract (small JSON,
// loaded with the page so agents, the validator and the gallery can read it)
// and its code and surface (loaded on first use).
import CONTRACT_SCHEMA from './schemas/v1/contract.schema.json' with { type: 'json' };
import pathOnMap from './path-on-map/contract.json' with { type: 'json' };
import { check } from './schema.js';
import { BASEMAP_VALUES, MERCATOR_LAT } from './surfaces/archives.js';

export const COMPONENTS = Object.freeze({
  'path-on-map': Object.freeze({ contract: pathOnMap, load: () => import('./path-on-map/index.js') }),
});
/** Surface kind -> its module (lazy) and the rules a contract and its data must meet on it. */
export const SURFACES = Object.freeze({
  map: Object.freeze({
    load: () => import('./surfaces/map.js'),
    rules: Object.freeze({ pointLat: MERCATOR_LAT, params: Object.freeze({ basemap: BASEMAP_VALUES }) }),
  }),
});
/** The registered component `id`, or undefined; never an inherited property. */
export const component = (id) => (typeof id === 'string' && Object.hasOwn(COMPONENTS, id) ? COMPONENTS[id] : undefined);
export const surfaceRules = (contract) => (contract && Object.hasOwn(SURFACES, contract.surface) ? SURFACES[contract.surface].rules : {});
/** Every contract, for listing and search. */
export const contracts = () => Object.values(COMPONENTS).map((c) => c.contract);

/**
 * A contract's problems, beyond its JSON Schema: reserved role names, an XY
 * `relativeTo` that names no surface parameter, settings whose default does
 * not meet their own schema, and surface-specific values (a map `basemap`
 * must be an archive the surface has).
 */
export function checkContract(contract) {
  const errors = check(contract, CONTRACT_SCHEMA);
  if (errors.length) return errors;
  const err = (at, message) => errors.push({ code: 'contract', at, message, help: '' });
  for (const [name, role] of Object.entries(contract.roles)) {
    if (name === 'rows' || name === 'groups') err(`roles.${name}`, `\`${name}\` is reserved for the binding's structure`);
    if (role.relativeTo !== undefined && !Object.hasOwn(contract.surfaceParams, role.relativeTo)) err(`roles.${name}.relativeTo`, `names no surface parameter: \`${role.relativeTo}\``);
    if (role.type === 'Enum' && !role.enum) err(`roles.${name}`, 'an Enum role needs `enum`');
    if (role.type === 'XY' && !role.frame) err(`roles.${name}`, 'an XY role needs a `frame`');
  }
  for (const field of ['surfaceParams', 'options']) {
    for (const [key, setting] of Object.entries(contract[field])) {
      for (const problem of check(setting.default, setting, `${field}.${key}.default`)) errors.push(problem);
    }
  }
  const allowed = surfaceRules(contract).params || {};
  for (const [key, values] of Object.entries(allowed)) {
    const setting = Object.hasOwn(contract.surfaceParams, key) ? contract.surfaceParams[key] : null;
    for (const value of setting?.enum || []) if (!values.includes(value)) err(`surfaceParams.${key}`, `${JSON.stringify(value)} is not a ${contract.surface} ${key} (it has ${values.join(', ')})`);
  }
  return errors;
}
