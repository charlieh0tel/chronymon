// The README's screenshots, of the synthetic database (`make
// screenshots`, through tools/with-web), into docs/images.

import { join } from "node:path";

import { chromium } from "@playwright/test";

import { routeCdn } from "./cdn.mjs";
import { FULL } from "./playwright.config.mjs";

const OUT = join(import.meta.dirname, "../../../../docs/images");
// Desktop, as the README's readers mostly see the pages; what shows in
// it is shot, not the whole page.
const VIEWPORT = { width: 1400, height: 950 };
// How long a page is given after its charts appear, for the readout and
// the strip to fill, in ms.
const SETTLE = 1500;
// Where across the first chart the cursor rests, for the readout.
const CURSOR = 0.6;
// Seconds since the last poll, as a page shows them: the synthetic
// readings end when they are written, and with no daemon writing more
// the strip would turn stale partway through the shots.
const POLL_AGE = 1;

// File, path, and what shows the page is drawn.
const SHOTS = [
  ["web-live.png", "/?last=86400", "#charts .uplot"],
  ["web-sources.png", "/sources?last=86400", "#charts .uplot"],
  ["web-refclocks.png", "/refclocks?last=86400", "#charts .uplot"],
  ["web-temperature.png", "/temperature?last=86400", "#scatter .uplot"],
  ["web-distribution.png", "/distribution?last=86400", "#figures .uplot"],
];

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: VIEWPORT });
  await routeCdn(page);
  await page.route("**/api/status", async (route) => {
    const response = await route.fetch();
    const status = await response.json();
    status.status.age = POLL_AGE;
    return route.fulfill({ response, json: status });
  });
  for (const [file, path, drawn] of SHOTS) {
    await page.goto(FULL + path);
    await page.waitForSelector(drawn);
    const chart = await page.locator(drawn).first().boundingBox();
    await page.mouse.move(chart.x + chart.width * CURSOR, chart.y + chart.height / 2);
    await page.waitForTimeout(SETTLE);
    await page.screenshot({ path: join(OUT, file) });
    console.log(file);
  }
} finally {
  await browser.close();
}
