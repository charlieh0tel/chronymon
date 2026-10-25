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
// chronymon-web's default database.
const DATABASE = "/var/lib/chronymon/chronymon.sqlite";

// Seconds of readings each page shows: six hours, quiet, after the
// synthetic run's step.
const LAST = 21600;

// A kiosk's display: landscape, as a television is.
const KIOSK_VIEWPORT = { width: 1920, height: 1080 };

// File, path, what shows the page is drawn, and the viewport if not
// VIEWPORT.
const SHOTS = [
  ["web-live.png", `/?last=${LAST}`, "#charts .uplot"],
  ["web-sources.png", `/sources?last=${LAST}`, "#charts .uplot"],
  ["web-refclocks.png", `/refclocks?last=${LAST}`, "#charts .uplot"],
  ["web-temperature.png", `/temperature?last=${LAST}`, "#scatter .uplot"],
  ["web-distribution.png", `/distribution?last=${LAST}`, "#figures .uplot"],
  ["web-kiosk.png", `/kiosk?last=${LAST}`, "#charts .uplot", KIOSK_VIEWPORT],
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
  // The colophon names the database where it is installed, not where
  // the shots' synthetic one happens to be.
  await page.route("**/api/about", async (route) => {
    const response = await route.fetch();
    const about = await response.json();
    about.database = DATABASE;
    return route.fulfill({ response, json: about });
  });
  for (const [file, path, drawn, viewport = VIEWPORT] of SHOTS) {
    await page.setViewportSize(viewport);
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
