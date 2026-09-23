const MONACO = 'https://cdn.jsdelivr.net/npm/monaco-editor@0.52.2/min';

function loadMonaco() {
  if (window.monaco) return Promise.resolve(window.monaco);
  window.MonacoEnvironment = {
    getWorker(_id, _label) {
      const source = `
        self.MonacoEnvironment = { baseUrl: '${MONACO}/' };
        importScripts('${MONACO}/vs/base/worker/workerMain.js');
      `;
      const url = `data:text/javascript;charset=utf-8,${encodeURIComponent(source)}`;
      return new Worker(url);
    },
  };
  return new Promise((resolve, reject) => {
    const require = window.require;
    if (!require) {
      reject(new Error('Monaco loader did not start'));
      return;
    }
    require.config({ paths: { vs: `${MONACO}/vs` } });
    require(['vs/editor/editor.main'], () => resolve(window.monaco), reject);
  });
}

const brackets = {
  comments: { lineComment: '//' },
  brackets: [['{', '}'], ['[', ']'], ['(', ')']],
  autoClosingPairs: [
    { open: '{', close: '}' },
    { open: '[', close: ']' },
    { open: '(', close: ')' },
    { open: '"', close: '"' },
  ],
};

function registerLanguages(monaco) {
  monaco.languages.register({ id: 'zega-schema' });
  monaco.languages.setLanguageConfiguration('zega-schema', brackets);
  monaco.languages.setMonarchTokensProvider('zega-schema', {
    tokenizer: {
      root: [
        [/\/\/.*$/, 'comment'],
        [/\b(schema|display|map|table|graph|timeline|Default|from)\b/, 'keyword'],
        [/\btype\b/, { token: 'keyword', next: '@typeName' }],
        [/\s+/, 'white'],
      ],
      typeName: [
        [/[A-Za-z_][\w]*/, { token: 'type', next: '@afterType' }],
        [/./, { token: '@rematch', next: '@root' }],
      ],
      afterType: [
        [/\{/, { token: 'delimiter.bracket', next: '@fields' }],
        [/./, { token: '@rematch', next: '@root' }],
      ],
      fields: [
        [/\/\/.*$/, 'comment'],
        [/\}/, { token: 'delimiter.bracket', next: '@root' }],
        [/"([^"\\]|\\.)*"/, 'string'],
        [/\b(String|Int|Float|Bool|Point|Vector)\b/, 'type'],
        [/->|<-/, { token: 'operator', next: '@target' }],
        [/[\[\]()|?:]/, 'delimiter'],
        [/[A-Z][\w]*/, 'type'],
        [/[a-z_][\w]*/, 'identifier'],
        [/\d+/, 'number'],
        [/\s+/, 'white'],
      ],
      target: [
        [/\(/, { token: 'delimiter', next: '@union' }],
        [/[A-Za-z_][\w]*/, { token: 'type', next: '@afterTarget' }],
        [/./, { token: '@rematch', next: '@fields' }],
      ],
      union: [
        [/\)/, { token: 'delimiter', next: '@afterTarget' }],
        [/\|/, 'delimiter'],
        [/[A-Za-z_][\w]*/, 'type'],
        [/\s+/, 'white'],
      ],
      afterTarget: [
        [/\[\]/, { token: 'delimiter', next: '@fields' }],
        [/./, { token: '@rematch', next: '@fields' }],
      ],
    },
  });

  monaco.languages.register({ id: 'zega-query' });
  monaco.languages.setLanguageConfiguration('zega-query', brackets);
  monaco.languages.setMonarchTokensProvider('zega-query', {
    tokenizer: {
      root: [
        [/\/\/.*$/, 'comment'],
        [/"([^"\\]|\\.)*"/, 'string'],
        [/\b(query|mutation|link|set|true|false|null|order|by|limit|distance|within_box|point|vector|near|similarity|score|exact)\b/, 'keyword'],
        [/\b(CONTAINS|STARTS|WITH|ENDS)\b/, 'keyword'],
        [/->|<-|>=|<=|<>|!=|&&|\|\|/, 'operator'],
        [/[<>]/, 'operator'],
        [/&[A-Za-z_][\w]*/, 'variable'],
        [/\*\d+\.\.\d+/, 'number'],
        [/\d+(?:\.\d+)?/, 'number'],
        [/[{}()[\]|,:=]/, 'delimiter'],
        [/[A-Z][\w]*/, 'type'],
        [/[a-z_][\w]*/, 'identifier'],
        [/\s+/, 'white'],
      ],
    },
  });

  monaco.languages.register({ id: 'zega-output' });
  monaco.languages.setMonarchTokensProvider('zega-output', {
    tokenizer: {
      root: [
        [/^error:.*$/, 'invalid'],
        [/^\s+\^+$/, 'invalid'],
        [/^\s+help:.*$/, 'comment'],
        [/^\s+(schema|query):\d+:\d+$/, 'number'],
      ],
    },
  });
}

const shared = {
  automaticLayout: true,
  minimap: { enabled: false },
  fontSize: 13,
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  lineHeight: 20,
  scrollBeyondLastLine: false,
  wordWrap: 'on',
  tabSize: 2,
  padding: { top: 8, bottom: 8 },
  fixedOverflowWidgets: true,
  glyphMargin: false,
  folding: false,
  lineNumbersMinChars: 3,
  renderLineHighlight: 'none',
  overviewRulerLanes: 2,
  hideCursorInOverviewRuler: true,
  scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
  theme: 'vs',
};

export async function createEditors({ schema, query }) {
  const monaco = await loadMonaco();
  registerLanguages(monaco);
  const schemaEditor = monaco.editor.create(document.getElementById('schema'), {
    ...shared,
    value: schema,
    language: 'zega-schema',
  });
  const queryEditor = monaco.editor.create(document.getElementById('query'), {
    ...shared,
    value: query,
    language: 'zega-query',
  });
  const readOnly = {
    ...shared,
    value: '',
    readOnly: true,
    domReadOnly: true,
    folding: true,
    foldingStrategy: 'indentation',
    showFoldingControls: 'always',
  };
  const outputEditor = monaco.editor.create(document.getElementById('output'), {
    ...readOnly,
    language: 'json',
  });
  const rawEditor = monaco.editor.create(document.getElementById('raw'), {
    ...readOnly,
    language: 'plaintext',
    wordWrap: 'off',
  });
  return { monaco, schema: schemaEditor, query: queryEditor, output: outputEditor, raw: rawEditor };
}
