// The README's screenshots, of the synthetic database (`make
// screenshots`, through tools/with-web), into docs/images.

import { join } from "node:path";

import { chromium } from "@playwright/test";

import { routeCdn } from "./cdn.mjs";
import { FULL } from "./playwright.config.mjs";

const OUT = join(import.meta.dirname, "../../../../docs/images");
// About as wide as GitHub shows a README, so the pictures show at their
// own size, at twice the pixels for high-density screens; what shows
// in it is shot, not the whole page.
const VIEWPORT = { width: 960, height: 800 };
const SCALE = 2;
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

// A kiosk's display: landscape, as a television is, shot at its own
// pixels, already twice the README's width.
const KIOSK = { viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 };
const PAGE = { viewport: VIEWPORT, deviceScaleFactor: SCALE };

// File, path, what shows the page is drawn, and the page's size if not
// PAGE.
const SHOTS = [
  ["web-live.png", `/?last=${LAST}`, "#charts .uplot"],
  ["web-sources.png", `/sources?last=${LAST}`, "#charts .uplot"],
  ["web-refclocks.png", `/refclocks?last=${LAST}`, "#charts .uplot"],
  ["web-temperature.png", `/temperature?last=${LAST}`, "#scatter .uplot"],
  ["web-distribution.png", `/distribution?last=${LAST}`, "#figures .uplot"],
  ["web-kiosk.png", `/kiosk?last=${LAST}`, "#charts .uplot", KIOSK],
];

const browser = await chromium.launch();
// A page of `size`, its answers adjusted for the shots.
async function newPage(size) {
  const page = await browser.newPage(size);
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
  return page;
}

try {
  for (const [file, path, drawn, size = PAGE] of SHOTS) {
    const page = await newPage(size);
    await page.goto(FULL + path);
    await page.waitForSelector(drawn);
    const chart = await page.locator(drawn).first().boundingBox();
    await page.mouse.move(chart.x + chart.width * CURSOR, chart.y + chart.height / 2);
    await page.waitForTimeout(SETTLE);
    await page.screenshot({ path: join(OUT, file) });
    await page.close();
    console.log(file);
  }
} finally {
  await browser.close();
}
