// Formatting a pane in place without disturbing the person typing in it.
//
// The formatter only moves whitespace, so a position is carried across by
// counting the non-whitespace characters before it: the cursor after the
// third `e` stays after the third `e`. The edit replaces only the part that
// changed, so markers, decorations and undo history outside it survive.

const space = (ch) => ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r';

/** The offset in `to` that sits after as many non-whitespace characters as `offset` does in `from`. */
export function carryOffset(from, offset, to) {
  let count = 0;
  for (let i = 0; i < offset; i++) if (!space(from[i])) count++;
  if (!count) return 0;
  for (let i = 0; i < to.length; i++) {
    if (!space(to[i]) && --count === 0) return i + 1;
  }
  return to.length;
}

/** The smallest single replacement that turns `before` into `after`. */
export function changedMiddle(before, after) {
  let start = 0;
  const limit = Math.min(before.length, after.length);
  while (start < limit && before[start] === after[start]) start++;
  let end = 0;
  while (end < limit - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  return { start, end: before.length - end, text: after.slice(start, after.length - end) };
}

/**
 * True when the text just before `offset` is whitespace the person typed on
 * purpose: a space before the next word, or a new line for the next field.
 * The formatter would remove it, and what they type next would join the
 * previous word (`name` + space, pause, `salary` gives `namesalary`).
 */
export function typingAfterSpace(text, offset) {
  return offset > 0 && space(text[offset - 1]);
}

/**
 * True when the source holds a `mutation { … }` block. Strings and line
 * comments are blanked first, so `"mutation {"` inside a value does not count.
 */
export function hasMutation(source) {
  return /(?:^|[^\w])mutation\s*\{/.test(code(source));
}

/**
 * Whether ZQL may write: any `mutation` block, including a sourced one
 * (`mutation csv [...] { ... }`), outside strings and comments. Broader than
 * hasMutation on purpose: a remote graph's snapshot is refreshed after a
 * possible write and never after a read (backend.js RemoteDatabase).
 */
export function mayWrite(source) {
  return /(?:^|[^\w])mutation\b/.test(code(source));
}

function code(source) {
  return source.replace(/"(?:[^"\\\n]|\\.)*"?|\/\/[^\n]*/g, ' ');
}

/**
 * Formats one Monaco editor in place when its text parses.
 *
 * `format` is the WASM formatter, which hands back unparseable text as it
 * is. Returns true when the text changed. With `history: false` (used on
 * load) the formatted text replaces the model's history, so undo never
 * reveals the unformatted source nobody typed.
 */
export function formatEditor(editor, format, { history = true } = {}) {
  const model = editor.getModel();
  const before = model.getValue();
  let after;
  try { after = format(before); } catch { return false; }
  if (typeof after !== 'string' || after === before) return false;
  if (!history) {
    model.setValue(after);
    return true;
  }
  const carry = (position) => carryOffset(before, model.getOffsetAt(position), after);
  // Anchor and head, not start and end, so a selection made upwards keeps its direction.
  const selections = (editor.getSelections() || []).map((selection) => ({
    anchor: carry({ lineNumber: selection.selectionStartLineNumber, column: selection.selectionStartColumn }),
    head: carry(selection.getPosition()),
  }));
  const scrollTop = editor.getScrollTop();
  const scrollLeft = editor.getScrollLeft();
  const { start, end, text } = changedMiddle(before, after);
  const range = {
    startLineNumber: model.getPositionAt(start).lineNumber,
    startColumn: model.getPositionAt(start).column,
    endLineNumber: model.getPositionAt(end).lineNumber,
    endColumn: model.getPositionAt(end).column,
  };
  editor.pushUndoStop();
  editor.executeEdits('zega.format', [{ range, text, forceMoveMarkers: false }], () =>
    selections.map(({ anchor, head }) => {
      const a = model.getPositionAt(anchor);
      const h = model.getPositionAt(head);
      return { selectionStartLineNumber: a.lineNumber, selectionStartColumn: a.column,
        positionLineNumber: h.lineNumber, positionColumn: h.column };
    }));
  editor.pushUndoStop();
  editor.setScrollTop(scrollTop);
  editor.setScrollLeft(scrollLeft);
  return true;
}
