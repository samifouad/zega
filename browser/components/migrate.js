// View-spec format migrations. A spec names its format (`"format": 1`); a
// spec without one is format 0, the first draft (zega#109's first commit),
// whose bindings were list paths: `{ path: "points[].at", value: "points[].speed" }`.
// Every older format the runtime has shipped is migrated here, forever, so a
// stored notebook entry or a shared link keeps opening.
//
// Breaking-change policy:
// - The view-spec *format* major changes only when the spec's own structure
//   changes (as 0 -> 1 did for bindings). Each bump adds a step below.
// - A *component's* major changes when an existing spec may stop binding
//   (a role renamed or retyped, an option removed or narrowed). The registry
//   keeps the previous major's contract and a `migrate(spec)` from it until no
//   supported notebook refers to it. Minors add (a role, an option, an enum
//   value); a spec written against a newer minor than the runtime has gets a
//   warning, and anything it uses that the runtime lacks is an error.
export const FORMAT = 1;

const LIST = /^(?:(\[\])|([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\[\])\.(.+)$/;

/** Format 0 -> 1: list paths become `rows` plus row fields. Returns a new spec. */
function from0(spec) {
  const binding = spec?.binding;
  if (!binding || typeof binding !== 'object' || Array.isArray(binding)) return { ...spec, format: 1 };
  // Null prototype: a role named `__proto__` stays a key (and is refused by validation), never a prototype.
  const out = Object.create(null);
  let rows = null;
  for (const [role, path] of Object.entries(binding)) {
    const match = typeof path === 'string' ? LIST.exec(path) : null;
    if (!match) { out[role] = path; continue; }
    const list = match[1] ? '.' : match[2];
    if (rows !== null && rows !== list) return { ...spec, format: 1 }; // two lists: nothing sound to migrate to; let validation explain
    rows = list;
    out[role] = match[3];
  }
  const { format, ...rest } = spec;
  return { format: 1, ...rest, binding: rows === null ? out : { rows, ...out } };
}

/**
 * The spec in the current format, and the format it was written in.
 * Unknown future formats are returned unchanged for validation to refuse.
 */
export function migrate(spec) {
  if (!spec || typeof spec !== 'object' || Array.isArray(spec)) return { spec, from: FORMAT };
  const from = Object.hasOwn(spec, 'format') ? spec.format : 0;
  if (from === 0) return { spec: from0(spec), from: 0 };
  return { spec, from };
}
