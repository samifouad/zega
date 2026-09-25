// The graph-component registry: every component's contract (small JSON,
// loaded with the page so agents, the validator and the gallery can read it)
// and its code and surface (loaded on first use).
import pathOnMap from './path-on-map/contract.json' with { type: 'json' };

export const COMPONENTS = {
  'path-on-map': { contract: pathOnMap, load: () => import('./path-on-map/index.js') },
};
export const SURFACES = {
  map: () => import('./surfaces/map.js'),
};
/** Every contract, for listing and search. */
export const contracts = () => Object.values(COMPONENTS).map((c) => c.contract);
