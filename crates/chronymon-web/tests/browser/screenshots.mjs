// The README's screenshots, of the synthetic database (`make
// screenshots`, through tools/with-web), into docs/images.

import { join } from "node:path";

import { chromium } from "@playwright/test";

import { routeCdn } from "./cdn.mjs";
import { FULL, GPSD_STOPPED_HOST } from "./playwright.config.mjs";

const OUT = join(import.meta.dirname, "../../../../docs/images");
// About as wide as GitHub shows a README, so the pictures show at their
// own size, at twice the pixels for high-density screens; what shows
// in it is shot, not the whole page.
const VIEWPORT = { width: 960, height: 800 };
const SCALE = 2;
// How long a page is given after its charts appear, for the readout and
// the strip to fill, in ms.
const SETTLE = 1500;
// Pixels of page kept above and below a clipped shot.
const EDGE = 12;
// Where across the first chart the cursor rests, for the readout.
const CURSOR = 0.6;
// Seconds since the last poll, as a page shows them: the synthetic
// readings end when they are written, and with no daemon writing more
// the strip would turn stale partway through the shots.
const POLL_AGE = 1;
// chronymon-web's default database.
const DATABASE = "/var/lib/chronymon/chronymon.sqlite";

// Seconds of readings each page shows: six hours, quiet, after the
// synthetic run's step; for the fit, the whole run.
const LAST = 21600;
const WHOLE_RUN = 172800;
// Seconds either side of the PPS's flapping and the fix's return the
// Refclocks shot shows, and where across its charts the pointer rests:
// in the pulses and the fix lost.
const AROUND = 1800;
const REFCLOCKS_POINT = 0.72;

// The Refclocks page over the synthetic run's PPS and receiver
// troubles, from the PPS flapping to the fix regained.
async function refclocksPath() {
  const events = await (await fetch(`${FULL}/api/events?from=0&limit=500`)).json();
  const at = (kind) => events.find((e) => e.kind === kind).at;
  return `/refclocks?from=${Math.floor(at("pps_flapping") - AROUND)}&to=${Math.ceil(at("fix_regained") + AROUND)}`;
}

// A kiosk's display: landscape, as a television is, shot at its own
// pixels, already twice the README's width.
const KIOSK = { viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 };
const PAGE = { viewport: VIEWPORT, deviceScaleFactor: SCALE };

// File, where (a path, or a function giving one), what shows the page
// is drawn, and optionally the page's size if not PAGE, a button to
// click once drawn, the titles of the first and last charts to shoot in
// place of the window, and where across them the pointer rests, or what
// the panel to shoot alone holds.
const SHOTS = [
  { file: "web-live.png", path: `/?last=${LAST}`, drawn: "#charts .uplot" },
  { file: "web-sources.png", path: `/sources?last=${LAST}`, drawn: "#charts .uplot" },
  // The PPS's pulses flapping and lost and the receiver's fix lost,
  // from the PPS's offset down to the satellites used.
  {
    file: "web-refclocks.png", path: refclocksPath, drawn: "#charts .uplot",
    clip: ["PPS: Offset", "Satellites used"], pointAt: REFCLOCKS_POINT,
  },
  // The scatter, its linear and quadratic fits over it, over the whole
  // run's swing of temperature.
  {
    file: "web-temperature.png", path: `/temperature?last=${WHOLE_RUN}`, drawn: "#scatter .uplot",
    panel: "#scatter",
  },
  {
    file: "web-temperature-fit.png", path: `/temperature?last=${WHOLE_RUN}`, drawn: "#scatter .uplot",
    click: "#fit",
  },
  { file: "web-distribution.png", path: `/distribution?last=${LAST}`, drawn: "#figures .uplot" },
  { file: "web-kiosk.png", path: `/kiosk?last=${LAST}`, drawn: "#charts .uplot", size: KIOSK },
  // A host whose gpsd stopped: the PPS locked to its NMEA, no samples.
  {
    file: "web-kiosk-gpsd-stopped.png", base: GPSD_STOPPED_HOST, path: "/kiosk",
    drawn: "#charts .uplot", size: KIOSK,
  },
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
  for (const { file, base = FULL, path, drawn, size = PAGE, click, clip, pointAt, panel } of SHOTS) {
    const page = await newPage(size);
    await page.goto(base + (typeof path === "function" ? await path() : path));
    await page.waitForSelector(drawn);
    const chart = await page.locator(drawn).first().boundingBox();
    await page.mouse.move(chart.x + chart.width * CURSOR, chart.y + chart.height / 2);
    await page.waitForTimeout(SETTLE);
    if (click) {
      await page.locator(click).click();
      await page.waitForTimeout(SETTLE);
    }
    if (clip) {
      const chart = (title) => page.locator("#charts .uplot", { has: page.locator(".u-title", { hasText: title }) });
      await page.waitForFunction(() => !running);
      // The window grown to hold the last, so it is drawn.
      const last = await chart(clip[1]).first().boundingBox();
      await page.setViewportSize({ width: size.viewport.width, height: Math.ceil(last.y + last.height) + EDGE });
      // The pointer in the span `pointAt` names, for its tip to say what it is.
      if (pointAt != null) {
        const over = await chart(clip[0]).first().locator(".u-over").boundingBox();
        await page.mouse.move(over.x + over.width * pointAt, over.y + over.height / 2);
        await page.waitForTimeout(SETTLE);
      }
      const [top, bottom] = await Promise.all(clip.map((title) => chart(title).first().boundingBox()));
      await page.screenshot({
        path: join(OUT, file),
        clip: { x: 0, y: top.y - EDGE, width: size.viewport.width, height: bottom.y + bottom.height - top.y + EDGE },
      });
    } else if (panel) {
      await page.locator(".panel", { has: page.locator(panel) }).screenshot({ path: join(OUT, file) });
    } else {
      await page.screenshot({ path: join(OUT, file) });
    }
    // A page polls on: its requests in flight are let go, not failed.
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.close();
    console.log(file);
  }
} finally {
  await browser.close();
}
