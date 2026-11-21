// Opt-in browser acceptance. The Python test owns the writer and HTTP server.
const assert = require('node:assert/strict');
const readline = require('node:readline');
const { chromium } = require(process.env.CUA_BENCH_PLAYWRIGHT || 'playwright');
const input = readline.createInterface({ input: process.stdin });
const nextLine = () => new Promise((resolve) => input.once('line', resolve));
(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CUA_BENCH_CHROME ? { executablePath: process.env.CUA_BENCH_CHROME } : {}),
  });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    // No external CDN or other internet access is allowed.
    await page.route('**/*', (route) =>
      new URL(route.request().url()).hostname === '127.0.0.1' ? route.continue() : route.abort()
    );
    await page.addInitScript(() => {
      const blob = Response.prototype.blob;
      Response.prototype.blob = function () {
        if (!this.url.includes('/preview/'))
          throw new Error('Whole attachment buffered in browser');
        return blob.call(this);
      };
    });
    await page.goto(process.argv[2]);
    if (process.argv[3] === 'related') {
      await page.locator('[data-sequence="0"]').click();
      await page.waitForFunction(() => document.querySelectorAll('#related button').length === 100);
      await page.getByRole('button', { name: 'Next related page', exact: true }).click();
      await page.locator('[data-related-sequence="150"]').click();
      await page.waitForFunction(
        () => document.getElementById('selected').textContent === '150 · evidence'
      );
      await page.waitForFunction(
        () => document.getElementById('page').textContent === 'Page 2 of 3'
      );
      await page.selectOption('#order', 'observed');
      await page.waitForFunction(
        () => document.getElementById('page').textContent === 'Page 1 of 3'
      );
      assert.equal(
        await page.locator('[aria-current="true"]').getAttribute('data-sequence'),
        '150'
      );
      await page.getByRole('button', { name: 'Next page', exact: true }).click();
      await page.waitForFunction(
        () => document.getElementById('page').textContent === 'Page 2 of 3'
      );
      assert.equal(await page.locator('#selected').innerText(), '150 · evidence');
      await page.getByRole('button', { name: 'Reconnect / refresh' }).click();
      await page.waitForFunction(
        () => document.getElementById('page').textContent === 'Page 1 of 3'
      );
      assert.equal(
        await page.locator('[aria-current="true"]').getAttribute('data-sequence'),
        '150'
      );
      assert.deepEqual(errors, []);
      console.log('passed');
      return;
    }
    if (process.argv[3] === 'download') {
      await page.locator('[data-sequence="0"]').click();
      const ready = page.waitForEvent('download');
      await page.getByRole('button', { name: 'Download', exact: true }).click();
      const download = await ready;
      const fs = require('node:fs'),
        crypto = require('node:crypto');
      const digest = crypto.createHash('sha256');
      let size = 0;
      for await (const chunk of fs.createReadStream(await download.path())) {
        size += chunk.length;
        digest.update(chunk);
      }
      assert.equal(size, Number(process.argv[4]));
      assert.equal(digest.digest('hex'), process.argv[5]);
      assert.deepEqual(errors, []);
      console.log('passed');
      return;
    }
    if (process.argv[3] === 'large') {
      await page.waitForFunction(() =>
        document.getElementById('count').textContent.includes('100000 events')
      );
      assert.equal(await page.locator('#events li').count(), 100);
      await page.getByRole('button', { name: 'Next page', exact: true }).click();
      await page.waitForFunction(
        () => document.getElementById('page').textContent === 'Page 2 of 1000'
      );
      await page.locator('[data-sequence="150"]').click();
      await page.waitForFunction(
        () => document.getElementById('selected').textContent === '150 · sample'
      );
      await page.selectOption('#order', 'observed');
      await page.waitForFunction(
        () => document.querySelector('[aria-current="true"]')?.dataset.sequence === '150'
      );
      const session = await page.context().newCDPSession(page);
      await session.send('Performance.enable');
      const { metrics } = await session.send('Performance.getMetrics');
      const heap = metrics.find((entry) => entry.name === 'JSHeapUsedSize').value;
      assert.ok(heap < 32 * 1024 * 1024, `Browser heap ${heap} exceeds budget`);
      assert.equal(await page.locator('#events li').count(), 100);
      assert.deepEqual(errors, []);
      console.log(
        `100000 events: browser heap ${(heap / 1024 ** 2).toFixed(2)} MiB; 100 rows rendered`
      );
      console.log('passed');
      return;
    }
    await page.waitForFunction(() => document.querySelectorAll('#events li').length === 2);
    await page.locator('[data-sequence="0"]').click();
    await page.locator('[data-related-sequence="1"]').click();
    await page.waitForFunction(
      () => document.getElementById('selected').textContent === '1 · request'
    );
    await page.locator('[data-related-sequence="0"]').click();
    await page.waitForFunction(
      () => document.getElementById('selected').textContent === '0 · response'
    );
    await page.selectOption('#order', 'observed');
    await page.waitForFunction(
      () => document.querySelector('#events button').dataset.sequence === '1'
    );
    assert.deepEqual(
      await page
        .locator('#events button')
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.sequence)),
      ['1', '0']
    );
    assert.equal(await page.locator('#selected').innerText(), '0 · response');
    await page.getByRole('button', { name: 'Preview', exact: true }).first().click();
    await page.waitForFunction(() => document.getElementById('imagePreview').naturalWidth === 32);
    assert.equal(await page.locator('#overlays rect').count(), 1);
    assert.equal(await page.locator('#overlays rect').getAttribute('x'), '3');
    await page.locator('#showOverlays').uncheck();
    assert.equal(await page.locator('#overlays rect').count(), 0);
    await page.locator('#showOverlays').check();
    await page.getByRole('button', { name: 'Previous image', exact: true }).click();
    await page.waitForFunction(
      () => document.getElementById('selected').textContent === '1 · request'
    );
    await page.getByRole('button', { name: 'Next image', exact: true }).click();
    await page.waitForFunction(
      () => document.getElementById('selected').textContent === '0 · response'
    );
    await page.waitForFunction(() => document.querySelectorAll('#overlays rect').length === 1);
    await page.getByRole('button', { name: 'Close', exact: true }).click();
    await page.getByRole('button', { name: 'Preview', exact: true }).last().click();
    await page.waitForFunction(() =>
      document.getElementById('textPreview').textContent.includes('<script>')
    );
    assert.equal(await page.evaluate(() => window.capturedScriptRan), undefined);
    await page.keyboard.press('Escape');
    const downloadReady = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Download', exact: true }).last().click();
    const download = await downloadReady;
    const fs = require('node:fs');
    assert.equal(
      fs.readFileSync(await download.path(), 'utf8'),
      '<script>window.capturedScriptRan=true</script>\n✓'
    );
    await page.getByRole('button', { name: 'Pause updates' }).click();
    const appendedAt = Date.now();
    console.log('append');
    await nextLine();
    await page.waitForFunction(() =>
      document.getElementById('count').textContent.includes('1 pending')
    );
    assert.ok(Date.now() - appendedAt < 2000, 'Append-to-poll exceeded two seconds');
    assert.equal(await page.locator('#events li').count(), 2);
    assert.equal(await page.locator('[data-related-sequence="2"]').count(), 0);
    await page.getByRole('button', { name: 'Resume updates' }).click();
    await page.waitForFunction(() => document.querySelectorAll('#events li').length === 3);
    assert.deepEqual(
      await page
        .locator('#events button')
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.sequence)),
      ['2', '1', '0']
    );
    assert.equal(await page.locator('#selected').innerText(), '0 · response');
    await page.locator('[data-related-sequence="2"]').waitFor();
    await page.context().setOffline(true);
    await page.waitForFunction(() =>
      document.getElementById('status').textContent.includes('Disconnected')
    );
    await page.context().setOffline(false);
    await page.getByRole('button', { name: 'Reconnect / refresh' }).click();
    await page.waitForFunction(() =>
      document.getElementById('status').textContent.includes('Following')
    );
    assert.equal(await page.locator('#events li').count(), 3);
    assert.equal(await page.locator('[data-related-sequence="2"]').count(), 1);
    await page.locator('#dataDetails').evaluate((node) => {
      node.open = false;
    });
    console.log('finish');
    await nextLine();
    await page.waitForFunction(() => document.getElementById('status').textContent === 'Completed');
    assert.equal(await page.locator('#events li').count(), 6);
    assert.deepEqual(
      await page
        .locator('#events button')
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.sequence)),
      ['2', '1', '0', '3', '5', '4']
    );
    assert.equal(await page.locator('#selected').innerText(), '0 · response');
    assert.equal(await page.locator('#dataDetails').evaluate((node) => node.open), false);
    assert.deepEqual(errors, []);
    if (process.env.CUA_BENCH_SCREENSHOT)
      await page.screenshot({ path: process.env.CUA_BENCH_SCREENSHOT, fullPage: true });
    console.log('passed');
  } finally {
    await browser.close();
    input.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
  input.close();
});
