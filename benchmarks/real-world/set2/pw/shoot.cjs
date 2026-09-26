// Screenshots a page frame by frame for ffmpeg to lay over the footage.
// The page's setTime(t) draws the moment and says whether anything is on
// screen; frames with nothing are one blank picture, hard-linked, so only
// frames with graphics cost a screenshot.
// Usage: node shoot.cjs <page.html> <width> <height> <fps> <frames> <outdir>
const { chromium } = require('playwright');
const { linkSync, mkdirSync } = require('node:fs');
const { resolve } = require('node:path');
(async () => {
  const [, , file, w, h, fps, frames, out] = process.argv;
  mkdirSync(out, { recursive: true });
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: Number(w), height: Number(h) } });
  await page.goto('file://' + resolve(file));
  await page.addStyleTag({ content: 'html,body{background:transparent!important}' });
  await page.evaluate(() => window.ready);
  await page.evaluate(() => window.setTime(-1));
  const blank = `${out}/blank.png`;
  await page.screenshot({ path: blank, omitBackground: true });
  for (let n = 0; n < Number(frames); n++) {
    const path = `${out}/${String(n).padStart(5, '0')}.png`;
    if (await page.evaluate((t) => window.setTime(t), n / Number(fps))) {
      await page.screenshot({ path, omitBackground: true });
    } else {
      linkSync(blank, path);
    }
  }
  await browser.close();
})();
