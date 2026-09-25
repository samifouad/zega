// On a phone the panes stack in one column (style.css, zegadb/zega#92). The
// long ones get a button to fold them away; it is only shown at phone width,
// and folding only applies there, so the desktop layout never changes.
const COLLAPSIBLE = ['output', 'schema', 'raw'];

for (const name of COLLAPSIBLE) {
  const pane = document.querySelector(`.pane[data-pane="${name}"]`);
  const heading = pane?.querySelector('h2');
  const body = pane?.querySelector('.editor');
  if (!heading || !body) continue;
  const toggle = document.createElement('button');
  toggle.type = 'button';
  toggle.className = 'pane-toggle';
  toggle.setAttribute('aria-controls', body.id);
  const show = (open) => {
    pane.classList.toggle('pane-collapsed', !open);
    toggle.setAttribute('aria-expanded', String(open));
    toggle.textContent = open ? 'hide' : 'show';
    toggle.setAttribute('aria-label', `${open ? 'Hide' : 'Show'} ${name}`);
  };
  toggle.addEventListener('click', () => show(pane.classList.contains('pane-collapsed')));
  show(true);
  heading.prepend(toggle);
}
