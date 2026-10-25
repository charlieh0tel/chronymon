// Every page, against the synthetic database (tools/with-web): it
// loads without an error, draws what it should, and its controls work;
// and against no database at all, where every page must say so.

import { expect, test } from "@playwright/test";

import { routeCdn } from "./cdn.mjs";
import { MISSING } from "./playwright.config.mjs";

const PAGES = [
  { path: "/", name: "Live" },
  { path: "/sources", name: "Sources" },
  { path: "/refclocks", name: "Refclocks" },
  { path: "/temperature", name: "Temperature" },
  { path: "/journal", name: "Journal" },
  { path: "/distribution", name: "Distribution" },
];

// Each test's page errors, collected while it runs: uncaught
// exceptions, console errors, and requests that failed.
const pageErrors = new Map();

test.beforeEach(async ({ page }, info) => {
  const errors = [];
  pageErrors.set(info.testId, errors);
  page.on("pageerror", (e) => errors.push(`exception: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(`console: ${m.text()}`);
  });
  page.on("requestfailed", (r) => errors.push(`request: ${r.url()} ${r.failure()?.errorText}`));
  await routeCdn(page);
});

test.afterEach(async ({}, info) => {
  expect(pageErrors.get(info.testId), "errors on the page").toEqual([]);
});

const charts = (page, within = "#charts") => page.locator(`${within} .uplot`);

for (const { path, name } of PAGES) {
  test(`${name}: loads, live, its nav entry current`, async ({ page }) => {
    await page.goto(path);
    // The synthetic data's status row is minutes old by now.
    await expect(page.locator("#status .pill").first()).toHaveText(/^(LIVE|STALE)$/);
    await expect(page.locator(`nav a[aria-current="page"]`)).toHaveText(name);
  });

  test(`${name}: with no database, says so`, async ({ page }) => {
    await page.goto(MISSING + path);
    await expect(page.locator("#status")).toContainText("no database");
  });
}

test("Live: the strip reads the clock", async ({ page }) => {
  await page.goto("/");
  const strip = page.locator("#status");
  for (const name of ["reference", "stratum", "last offset", "frequency", "skew", "leap"]) {
    await expect(strip.locator(".stat .k", { hasText: name }).first()).toBeVisible();
  }
  // The synthetic clock is disciplined by its PPS refclock.
  await expect(strip.locator(".stat", { hasText: "reference" })).toContainText("PPS");
});

test("Live: charts, a readout under the cursor, and the events", async ({ page }) => {
  await page.goto("/?last=86400");
  await expect(charts(page).first()).toBeVisible();
  expect(await charts(page).count()).toBeGreaterThan(1);
  const over = charts(page).first().locator(".u-over");
  const box = await over.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await expect(page.locator("#readout")).not.toBeEmpty();
  await expect(page.locator("#journal")).toContainText("step");
});

test("Live: a column unticked leaves its chart", async ({ page }) => {
  await page.goto("/?last=86400");
  await expect(charts(page).first()).toBeVisible();
  const before = await charts(page).count();
  await page.locator("#columns input[data-col]:checked").first().uncheck();
  await expect(charts(page)).toHaveCount(before - 1);
});

test("Live: chronyd's log fills where the daemon has no rows", async ({ page }) => {
  await page.goto("/?last=all");
  await expect(charts(page).first()).toBeVisible();
  const over = charts(page).first().locator(".u-over");
  const box = await over.boundingBox();
  // The far left of "all" is before the daemon's first poll.
  await page.mouse.move(box.x + 4, box.y + box.height / 2);
  await expect(page.locator("#readout")).toContainText("from chronyd's log");
});

test("Live: the range buttons move the window and the address", async ({ page }) => {
  await page.goto("/?last=86400");
  await expect(charts(page).first()).toBeVisible();
  const shown = page.locator("#range-shown");
  const before = await shown.textContent();
  await page.locator('#ranges button[data-last="3600"]').click();
  await expect(page.locator('#ranges button[data-last="3600"]')).toHaveAttribute("aria-pressed", "true");
  await expect(shown).not.toHaveText(before);
  expect(new URL(page.url()).searchParams.get("last")).toBe("3600");
});

test("Live: an event centers the charts on it", async ({ page }) => {
  // A day, which holds the synthetic run's step.
  await page.goto("/?last=86400");
  await expect(charts(page).first()).toBeVisible();
  const before = await page.locator("#range-shown").textContent();
  await page.locator("#journal .note-link").first().click();
  await expect(page.locator("#range-shown")).not.toHaveText(before);
});

test("Sources: a row per source, overlays, and a link to each", async ({ page }) => {
  await page.goto("/sources?last=86400");
  const rows = page.locator("#sources tbody tr");
  await expect(rows).toHaveCount(3);
  for (const name of ["PPS", "ntp1.example.net", "ntp2.example.org"]) {
    await expect(page.locator("#sources")).toContainText(name);
  }
  await expect(charts(page).first()).toBeVisible();
  await page.locator("#sources a", { hasText: "ntp1.example.net" }).first().click();
  await expect(page).toHaveURL(/\/source\?/);
  await expect(page.locator("#heading")).toContainText("ntp1.example.net");
  await expect(charts(page).first()).toBeVisible();
});

test("Refclocks: each refclock's offsets", async ({ page }) => {
  await page.goto("/refclocks?last=86400");
  await expect(page.locator("#refclocks")).toContainText("PPS");
  await expect(charts(page).first()).toBeVisible();
});

test("Temperature: the series and the scatter", async ({ page }) => {
  await page.goto("/temperature?last=86400");
  await expect(charts(page).first()).toBeVisible();
  await expect(page.locator("#scatter .uplot")).toBeVisible();
  await expect(page.locator("#figures")).not.toBeEmpty();
});

test("Journal: the synthetic run's events", async ({ page }) => {
  await page.goto("/journal?last=all");
  const events = page.locator("#events");
  for (const what of [
    "host booted",
    "chronymond started",
    "chronyd down",
    "chronyd up",
    "chronyd restarted",
    "clock stepped",
  ]) {
    await expect(events).toContainText(what);
  }
});

// A kiosk's display: landscape, as a television is.
const KIOSK_VIEWPORT = { width: 1920, height: 1080 };

// Whether the page fits its window, with nothing to scroll.
const fits = (page) => page.evaluate(() =>
  Math.max(document.documentElement.scrollHeight, document.body.scrollHeight) <= innerHeight &&
  Math.max(document.documentElement.scrollWidth, document.body.scrollWidth) <= innerWidth);

test("Kiosk: the clock's state, its sources, events and charts, on one screen", async ({ page }) => {
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk?last=86400");
  // The synthetic data's status row is minutes old by now.
  await expect(page.locator("#kiosk-mode")).toHaveText(/^(synchronized|stale)$/);
  await expect(page.locator("#kiosk-reference")).toHaveText("PPS");
  await expect(page.locator("#kiosk-stratum")).toHaveText("stratum 1");
  for (const name of ["last offset", "frequency", "skew", "leap"]) {
    await expect(page.locator("#kiosk-tiles .tile .k", { hasText: name }).first()).toBeVisible();
  }
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(3);
  await expect(page.locator("#kiosk-source-rows")).toContainText("ntp1.example.net");
  await expect(page.locator("#kiosk-event-rows")).toContainText("clock stepped");
  await expect(charts(page)).toHaveCount(4);
  await expect(page.locator("nav")).toHaveCount(0);
  expect(await fits(page)).toBe(true);
});

test("Kiosk: never changes its address, and redraws in place", async ({ page }) => {
  await page.setViewportSize(KIOSK_VIEWPORT);
  const address = "/kiosk?last=86400";
  await page.goto(address);
  await expect(charts(page)).toHaveCount(4);
  const length = await page.evaluate(() => history.length);
  await page.evaluate(() => {
    window.first = [...document.querySelectorAll("#charts .uplot"), ...$("kiosk-tiles").children];
  });
  await page.evaluate(() => renew());
  await page.waitForFunction(() => !running);
  // Two status polls.
  await page.waitForTimeout(2500);
  expect(await page.evaluate(() =>
    [...document.querySelectorAll("#charts .uplot"), ...$("kiosk-tiles").children]
      .every((el, n) => el === first[n]))).toBe(true);
  expect(new URL(page.url()).pathname + new URL(page.url()).search).toBe(address);
  expect(await page.evaluate(() => history.length)).toBe(length);
});

test("Kiosk: scaled down to fit a smaller screen", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/kiosk");
  await expect(charts(page)).toHaveCount(4);
  const unit = await page.evaluate(() => parseFloat(getComputedStyle(document.body).getPropertyValue("--u")));
  expect(unit).toBeCloseTo(720 / 1080, 2);
  expect(await fits(page)).toBe(true);
});

test("Kiosk: laid out inside the inset an overscanning display asks for", async ({ page }) => {
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk?inset=2,4");
  const [left, top, unit] = await page.evaluate(() => {
    const style = getComputedStyle(document.body);
    return [style.paddingLeft, style.paddingTop, style.getPropertyValue("--u")].map(parseFloat);
  });
  // Its own margin, in kiosk pixels, then the inset; and less room
  // than full size inside it, so scaled down.
  expect(unit).toBeLessThan(1);
  expect(left).toBeCloseTo(12 * unit + 0.02 * KIOSK_VIEWPORT.width, 0);
  expect(top).toBeCloseTo(12 * unit + 0.04 * KIOSK_VIEWPORT.height, 0);
  expect(await fits(page)).toBe(true);
});

test("Kiosk: reloaded, paints the last state before the first poll answers", async ({ page }) => {
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk");
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(3);
  await expect(page.locator("#kiosk-event-rows")).toContainText("clock stepped");
  // How long the server's answers are held back after the reload, in ms.
  const held = 3000;
  await page.route(/\/api\/(status|sources|events)/, async (route) => {
    await new Promise((done) => setTimeout(done, held));
    await route.continue();
  });
  await page.reload({ waitUntil: "domcontentloaded" });
  const head = page.locator("#kiosk-head");
  await expect(head).toContainText("LAST SEEN", { timeout: held / 2 });
  await expect(page.locator("#kiosk-reference")).toHaveText("PPS");
  await expect(page.locator("#kiosk-tiles .tile")).toHaveCount(8);
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(3);
  await expect(page.locator("#kiosk-event-rows")).toContainText("clock stepped");
  await expect(head).not.toContainText("LAST SEEN", { timeout: 3 * held });
});

test("Kiosk: works with storage refused", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", {
      get() {
        throw new Error("storage refused");
      },
    });
  });
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk?debug");
  await expect(page.locator("#kiosk-mode")).toHaveText(/^(synchronized|stale)$/);
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(3);
  await expect(charts(page)).toHaveCount(4);
  await expect(page.locator("#kiosk-foot")).toContainText("load ? (navigate)");
});

test("Kiosk: with debug, the foot counts loads and says how each came about", async ({ page }) => {
  await page.goto("/kiosk");
  await expect(page.locator("#kiosk-foot")).toBeHidden();
  await page.goto("/kiosk?debug");
  const foot = page.locator("#kiosk-foot");
  await expect(foot).toContainText(/^load 1 \(navigate\), up .*, Chrome \d+, \d+x\d+ at /);
  await page.reload();
  await expect(foot).toContainText("load 2 (reload)");
  await page.evaluate(() => setTimeout(() => {
    throw new Error("thrown for the test");
  }));
  await expect(foot).toContainText("error: Uncaught Error: thrown for the test");
  // That error is the test's own.
  const errors = pageErrors.get(test.info().testId);
  pageErrors.set(test.info().testId, errors.filter((e) => !e.includes("thrown for the test")));
});

test("Kiosk: with no database, says so", async ({ page }) => {
  await page.goto(MISSING + "/kiosk");
  await expect(page.locator("#kiosk-mode")).toHaveText("no data");
  await expect(page.locator("#kiosk-update")).toContainText("no database");
});

test("Distribution: percentiles and a histogram", async ({ page }) => {
  await page.goto("/distribution?last=86400");
  await expect(page.locator("#figures .uplot").first()).toBeVisible();
  await expect(page.locator("#figures")).toContainText("%");
});
