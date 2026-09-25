// Planar frames for XY roles: one registry, so every component on a rink,
// court, field or pitch agrees on its extents and unit. A role names its
// frame (`"frame": "rink"`) and takes both axes' ranges and the unit from
// here. Origins are the surface's centre (rink, court) or a corner (field,
// pitch), as the extents show. An XY role `relativeTo` a reference (e.g. the
// line of scrimmage) measures x from it, so its x range is ± the frame's
// length rather than the frame's own x extent.
export const FRAMES = Object.freeze({
  // NHL: 200 x 85 ft, centre ice at the origin.
  rink: Object.freeze({ unit: 'ft', x: Object.freeze([-100, 100]), y: Object.freeze([-42.5, 42.5]) }),
  // NBA: 94 x 50 ft, centre court at the origin.
  court: Object.freeze({ unit: 'ft', x: Object.freeze([-47, 47]), y: Object.freeze([-25, 25]) }),
  // NFL: 120 yd (with end zones) x 53.3 yd, from one back corner.
  field: Object.freeze({ unit: 'yd', x: Object.freeze([0, 120]), y: Object.freeze([0, 53.3]) }),
  // Football (soccer), IFAB's preferred 105 x 68 m, from one corner.
  pitch: Object.freeze({ unit: 'm', x: Object.freeze([0, 105]), y: Object.freeze([0, 68]) }),
});
export const frame = (name) => (typeof name === 'string' && Object.hasOwn(FRAMES, name) ? FRAMES[name] : undefined);
/** A frame's x and y ranges for a role: relative x spans ± the frame's length. */
export function extents(role) {
  const f = frame(role.frame);
  if (!f) return null;
  const length = f.x[1] - f.x[0];
  return { unit: f.unit, x: role.relativeTo ? [-length, length] : f.x, y: f.y };
}
