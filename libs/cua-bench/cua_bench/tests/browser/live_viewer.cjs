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
    await page.goto(process.argv[2]);
    await page.waitForFunction(() => document.querySelectorAll('#events li').length === 2);
    await page.locator('[data-sequence="0"]').click();
    await page.selectOption('#order', 'observed');
    assert.deepEqual(
      await page
        .locator('#events button')
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.sequence)),
      ['1', '0']
    );
    assert.equal(await page.locator('#selected').innerText(), '0 · response');
    await page.getByRole('button', { name: 'Preview', exact: true }).first().click();
    await page.waitForFunction(() => document.getElementById('imagePreview').naturalWidth === 32);
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
    console.log('append');
    await nextLine();
    await page.waitForFunction(() =>
      document.getElementById('count').textContent.includes('1 pending')
    );
    assert.equal(await page.locator('#events li').count(), 2);
    await page.getByRole('button', { name: 'Resume updates' }).click();
    await page.waitForFunction(() => document.querySelectorAll('#events li').length === 3);
    assert.deepEqual(
      await page
        .locator('#events button')
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.sequence)),
      ['2', '1', '0']
    );
    assert.equal(await page.locator('#selected').innerText(), '0 · response');
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
    console.log('finish');
    await nextLine();
    await page.waitForFunction(() => document.getElementById('status').textContent === 'Completed');
    assert.equal(await page.locator('#events li').count(), 3);
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
