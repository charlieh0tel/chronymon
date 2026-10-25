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

test("Distribution: percentiles and a histogram", async ({ page }) => {
  await page.goto("/distribution?last=86400");
  await expect(page.locator("#figures .uplot").first()).toBeVisible();
  await expect(page.locator("#figures")).toContainText("%");
});
