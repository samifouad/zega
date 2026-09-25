// The canvas's gear: the current component's advertised parameters as
// controls. Nothing here names a component or a parameter — every control is
// generated from the contract's `surfaceParams` and `options` descriptors (a
// JSON Schema with a default), so a component that advertises a parameter
// gets a control for it, and one that does not, does not.
//
//   enum            -> <select> of its values
//   boolean         -> checkbox
//   number/integer  -> range slider when it has a minimum and maximum, else a number field
//   anything else   -> a JSON text field, checked by the validator on change

const SECTIONS = [['surface', 'surfaceParams', 'surface'], ['options', 'options', 'options']];

/** One control for `descriptor`, showing `value`; `commit(next)` on change. */
export function controlFor(name, descriptor, value, commit) {
  const types = [descriptor.type].flat().filter(Boolean);
  let input;
  if (descriptor.enum) {
    input = document.createElement('select');
    for (const option of descriptor.enum) {
      const element = document.createElement('option');
      element.value = JSON.stringify(option);
      element.textContent = String(option);
      input.append(element);
    }
    input.value = JSON.stringify(value);
    input.onchange = () => commit(JSON.parse(input.value));
  } else if (types.length === 1 && types[0] === 'boolean') {
    input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = value;
    input.onchange = () => commit(input.checked);
  } else if (types.length === 1 && (types[0] === 'number' || types[0] === 'integer')) {
    input = document.createElement('input');
    const ranged = descriptor.minimum !== undefined && descriptor.maximum !== undefined;
    input.type = ranged ? 'range' : 'number';
    if (descriptor.minimum !== undefined) input.min = String(descriptor.minimum);
    if (descriptor.maximum !== undefined) input.max = String(descriptor.maximum);
    input.step = types[0] === 'integer' ? '1' : ranged ? String((descriptor.maximum - descriptor.minimum) / 100 >= 1 ? 1 : 0.1) : 'any';
    input.value = String(value);
    input.onchange = () => commit(Number(input.value));
  } else {
    input = document.createElement('input');
    input.type = 'text';
    input.spellcheck = false;
    input.value = JSON.stringify(value);
    input.onchange = () => {
      let parsed;
      try { parsed = JSON.parse(input.value); } catch { input.setCustomValidity('not JSON'); input.reportValidity(); return; }
      input.setCustomValidity('');
      commit(parsed);
    };
  }
  input.name = name;
  input.dataset.param = name;
  input.setAttribute('aria-label', name);
  return input;
}

/** The gear button and its panel, in `host`. `onChange(section, key, value)` on every edit. */
export function createGear(host, onChange) {
  const root = document.createElement('div');
  root.className = 'zc-gear';
  root.hidden = true;
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'zc-gear-button';
  button.textContent = '⚙';
  button.title = 'view settings';
  button.setAttribute('aria-label', 'view settings');
  button.setAttribute('aria-expanded', 'false');
  const panel = document.createElement('form');
  panel.className = 'zc-gear-panel';
  panel.hidden = true;
  panel.onsubmit = (event) => event.preventDefault();
  const error = document.createElement('pre');
  error.className = 'zc-gear-error';
  error.setAttribute('role', 'status');
  error.hidden = true;
  button.onclick = () => {
    panel.hidden = !panel.hidden;
    button.setAttribute('aria-expanded', String(!panel.hidden));
  };
  root.append(button, panel);
  host.append(root);

  return {
    element: root,
    /** Show the controls `contract` advertises, set to `spec`'s values. */
    show(contract, spec) {
      root.hidden = false;
      const rows = [];
      for (const [section, field, title] of SECTIONS) {
        const descriptors = contract[field] || {};
        if (!Object.keys(descriptors).length) continue;
        const heading = document.createElement('div');
        heading.className = 'zc-gear-section';
        heading.textContent = title;
        rows.push(heading);
        for (const [key, descriptor] of Object.entries(descriptors)) {
          const row = document.createElement('label');
          row.className = 'zc-gear-row';
          row.title = descriptor.description || '';
          const name = document.createElement('span');
          name.textContent = key;
          const value = spec[section][key];
          const shown = document.createElement('b');
          shown.textContent = typeof value === 'number' ? String(value) : '';
          const input = controlFor(`${section}.${key}`, descriptor, value, (next) => onChange(section, key, next));
          if (input.type === 'range') input.oninput = () => { shown.textContent = input.value; };
          row.append(name, input, shown);
          rows.push(row);
        }
      }
      error.hidden = true;
      panel.replaceChildren(...rows, error);
    },
    hide() { root.hidden = true; panel.hidden = true; button.setAttribute('aria-expanded', 'false'); },
    error(message) { error.textContent = message; error.hidden = !message; },
  };
}
