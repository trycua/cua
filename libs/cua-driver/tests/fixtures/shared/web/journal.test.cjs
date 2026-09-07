const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const { runInNewContext } = require('node:vm');

test('journal samples live scroll geometry without an event or DOM mutation', () => {
  const html = readFileSync(join(__dirname, 'index.html'), 'utf8');
  const source = html.match(/\(function startFixtureJournal\(\) \{[\s\S]*?\n  \}\)\(\);/);
  assert.ok(source, 'shared fixture journal exists');
  const scroll = { dataset: { cuaId: 'scroll-tall' }, textContent: 'contents', scrollTop: 208, clientHeight: 128 };
  const mirror = { dataset: { cuaId: 'lbl-scroll-offset' }, textContent: 'scroll_offset=104', scrollTop: 0, clientHeight: 16 };
  const field = { dataset: { cuaId: 'input' }, textContent: '', value: '42', checked: false, scrollTop: 0, clientHeight: 20 };
  const readonly = value => new Proxy(value, {
    get(target, key) {
      const result = target[key];
      return result && typeof result === 'object' ? readonly(result) : result;
    },
    set() { assert.fail('journal must not mutate DOM state'); },
    deleteProperty() { assert.fail('journal must not delete DOM state'); },
    defineProperty() { assert.fail('journal must not define DOM state'); },
  });
  const publications = [];
  const timers = [];
  let periodic;
  runInNewContext(source[0], {
    document: {
      body: {},
      querySelectorAll(selector) {
        assert.equal(selector, '[data-cua-id]');
        return [scroll, mirror, field].map(readonly);
      },
      addEventListener() {},
    },
    window: {
      cuaE2E: {
        journalUrl: 'http://fixture.invalid',
        publishFixtureState(state) { publications.push(JSON.parse(JSON.stringify(state))); },
      },
      addEventListener(name, callback) { assert.equal(name, 'load'); callback(); },
    },
    MutationObserver: class { observe() {} },
    setTimeout(callback, delay) { assert.equal(delay, 0); timers.push(callback); },
    setInterval(callback, delay) { assert.equal(delay, 250); periodic = callback; },
  });
  timers.shift()();
  assert.deepEqual(publications[0]['scroll-tall'], { text: 'contents', scrollTop: 208, clientHeight: 128 });
  assert.deepEqual(publications[0]['lbl-scroll-offset'], { text: 'scroll_offset=104' });
  assert.deepEqual(publications[0].input, { text: '', value: '42', checked: false });
  scroll.scrollTop = 320;
  periodic();
  timers.shift()();
  assert.equal(publications[1]['scroll-tall'].scrollTop, 320);
  assert.deepEqual(publications[1]['lbl-scroll-offset'], { text: 'scroll_offset=104' });
});
