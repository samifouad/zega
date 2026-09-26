// "Graph id or URL" in Connect to remote graph: parseGraphTarget (backend.js)
// turns what was pasted into the id, or refuses it. Pure: no page needed.
import { test, expect } from '@playwright/test';
import { parseGraphTarget } from '../backend.js';

const ID = 'ndry883u63ndp84ve7';

test('accepts a bare id, a router URL in every shape, and the graph hostname', () => {
  const accepted = [
    [ID, ID],
    [`  ${ID}\n`, ID],
    ['my-movies-x7k2m3q', 'my-movies-x7k2m3q'], // an old slug id
    [`https://api.zega.dev/g/${ID}/zql`, ID],
    [`https://api.zega.dev/g/${ID}/graph`, ID],
    [`https://api.zega.dev/g/${ID}`, ID],
    [`https://api.zega.dev/g/${ID}/`, ID],
    [`http://api.zega.dev/g/${ID}/zql`, ID],
    [`api.zega.dev/g/${ID}/zql`, ID],
    [`  api.zega.dev/g/${ID}  `, ID],
    [`https://api.zega.dev/g/${ID}/zql?explain=1`, ID],
    [`https://api.zega.dev/g/${ID}/graph#nodes`, ID],
    [`HTTPS://API.ZEGA.DEV/g/${ID}/zql`, ID],
    [`https://api.zega.dev/g/my-movies-x7k2m3q/zql`, 'my-movies-x7k2m3q'],
    [`https://${ID}.zegadb.com/`, ID],
    [`https://${ID}.zegadb.com/zql?x=1#y`, ID],
    [`${ID}.zegadb.com`, ID],
  ];
  for (const [input, id] of accepted) expect(parseGraphTarget(input), JSON.stringify(input)).toBe(id);
});

test('refuses other hosts, lookalikes and malformed ids, naming the problem', () => {
  const hosts = [
    `https://evil.example/g/${ID}/zql`,
    `https://api.zega.dev.evil.com/g/${ID}/zql`,
    `api.zega.dev.evil.com/g/${ID}`,
    `https://evil-api.zega.dev/g/${ID}/zql`,
    `https://apizega.dev/g/${ID}`,
    `https://explorer.zega.dev/g/${ID}`,
    `https://zega.dev/g/${ID}`,
    `https://${ID}.zegadb.com.evil.com/`,
    `https://a.${ID}.zegadb.com/`,
    `https://${ID}.zega.dev/`,
    `https://api.zega.dev@evil.com/g/${ID}`,
  ];
  for (const input of hosts) expect(() => parseGraphTarget(input), input).toThrow(/Only api\.zega\.dev\/g\/<id> or <id>\.zegadb\.com URLs/);
  expect(() => parseGraphTarget(`https://user:pw@api.zega.dev/g/${ID}`)).toThrow(/Only api\.zega\.dev/);
  expect(() => parseGraphTarget(`https://api.zega.dev:8443/g/${ID}`)).toThrow(/Only api\.zega\.dev/);
  expect(() => parseGraphTarget('https://api.zega.dev/v1/graphs')).toThrow('An api.zega.dev URL names its graph as /g/<id>.');
  expect(() => parseGraphTarget(`ftp://api.zega.dev/g/${ID}`)).toThrow('A graph URL starts with https://.');
  for (const bad of ['', '   ', 'Has Caps', 'under_score', 'x'.repeat(41), 'https://api.zega.dev/g/UPPER/zql', 'https://api.zega.dev/g/a%2Fb/zql', 'https://api.zega.dev/g/%E0%A4%A/zql']) {
    expect(() => parseGraphTarget(bad), JSON.stringify(bad)).toThrow();
  }
  expect(() => parseGraphTarget('Has Caps')).toThrow('A graph id is lowercase letters, digits and dashes.');
});
