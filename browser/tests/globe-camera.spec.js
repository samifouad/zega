import { test, expect } from './offline.js';
import { globe as openGlobe, map, idle, setEditor } from './globe-helpers.js';

// zega#100: once the reader has touched the globe's camera (dragged it,
// scroll-zoomed it, tilted it), it is theirs. Nothing here should snap it
// back: not a resize (a real one, or the kind a mobile browser's address
// bar makes by changing the viewport height), and not a query re-run that
// left the stored graph and the view unchanged. The bug lived in the padding
// `frame()` uses to keep a tilted planet vertically centred: it recomputed
// that padding from the *current* zoom/pitch on every zoomend/pitchend —
// including the reader's own scroll-zoom or drag — which visibly shifts the
// globe on screen even though centre/zoom themselves are untouched. So the
// camera snapshot below includes padding: it is as much "where the globe
// sits" as centre and zoom are.
const SCHEMA = `schema {
  type Country { name: String iso: String<iso2> }
  type City { name: String at: Point }
  display {
    globe(@zoom: 2, @tilt: 20, @center: @point(50, -60)) { Country, City } : Default
    table
  }
}
mutation { Country(name: "Canada" && iso: "CA") { name } }
mutation { Country(name: "Japan" && iso: "JP") { name } }
mutation { City(name: "Calgary" && at: @point(51.05, -114.07)) { name } }`;

const snapshot = (page) => map(page, `
  const c = map.getCenter();
  return {
    lng: +c.lng.toFixed(5), lat: +c.lat.toFixed(5), zoom: +map.getZoom().toFixed(5),
    pitch: +map.getPitch().toFixed(5), bearing: +map.getBearing().toFixed(5), padding: map.getPadding(),
  };
`);

// A left-drag, the same gesture a reader makes to spin the globe: real mouse
// events, so MapLibre's drag handler sees a user gesture (an `originalEvent`)
// and not one of the module's own programmatic camera calls. The canvas is
// scrolled into view first: off-screen (as it is at narrow widths until the
// pane is reached) a synthetic mouse event at its box coordinates lands on
// nothing.
async function drag(page, dx = 160, dy = 70) {
  await page.locator('.maplibregl-canvas').scrollIntoViewIfNeeded();
  const box = await page.locator('.maplibregl-canvas').boundingBox();
  const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
  await page.mouse.move(cx, cy);
  await page.mouse.down();
  await page.mouse.move(cx + dx, cy + dy, { steps: 20 });
  await page.mouse.up();
  await idle(page);
  await page.waitForTimeout(200);
}

test('a resize after a drag keeps the camera the reader set', async ({ page }) => {
  await openGlobe(page, SCHEMA);
  await drag(page);
  const before = await snapshot(page);
  // The reader's drag actually moved the camera off the schema's default.
  expect(before.lng).not.toBeCloseTo(-60, 1);

  // A resize the way a phone's address bar makes one: same width, shorter
  // viewport. One direction only — a round trip could land back on the same
  // padding by coincidence and hide a snap in between.
  const size = page.viewportSize();
  await page.setViewportSize({ width: size.width, height: size.height - 160 });
  await page.waitForTimeout(300);

  const after = await snapshot(page);
  expect(after.lng).toBeCloseTo(before.lng, 4);
  expect(after.lat).toBeCloseTo(before.lat, 4);
  expect(after.zoom).toBeCloseTo(before.zoom, 4);
  expect(after.pitch).toBe(before.pitch);
  expect(after.bearing).toBe(before.bearing);
  expect(after.padding).toEqual(before.padding);
});

test('a query re-run that leaves the stored graph unchanged keeps the camera', async ({ page }) => {
  await openGlobe(page, SCHEMA);
  // A real read query, run through auto-run (not the Run button, which
  // re-applies the schema's mutations): editing it again re-runs it against
  // the same stored graph, with nothing written.
  await setEditor(page, 'query', 'Country { name }');
  await page.waitForTimeout(500);
  await idle(page);
  await drag(page);
  const before = await snapshot(page);
  expect(before.lng).not.toBeCloseTo(-60, 1);

  // Re-run with a change that does not touch the query's meaning or the
  // stored data: the same auto-run debounce a reader's own re-run goes through.
  await setEditor(page, 'query', 'Country { name }\n');
  await page.waitForTimeout(500);
  await idle(page);

  const after = await snapshot(page);
  expect(after).toEqual(before);
});

test('at 390px, resizes the way page scroll makes on a phone do not move the globe', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openGlobe(page, SCHEMA);
  await drag(page, 60, 30);
  const before = await snapshot(page);

  // A few address-bar-style height changes in a row, as a reader scrolls:
  // checked after each one, since a bug that snaps and un-snaps could look
  // fine only at the very end. (The view pane is `min(70vh, 520px)`, so the
  // heights need to cross below the 520px cap to actually resize the pane.)
  for (const height of [700, 650, 600, 844]) {
    await page.setViewportSize({ width: 390, height });
    await page.waitForTimeout(200);
    const at = await snapshot(page);
    expect(at.lng).toBeCloseTo(before.lng, 4);
    expect(at.lat).toBeCloseTo(before.lat, 4);
    expect(at.zoom).toBeCloseTo(before.zoom, 4);
    expect(at.padding).toEqual(before.padding);
  }
});
