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

// The synthetic sources: chronyd's now, and the removed ntp3.
const SOURCES = [
  "PPS", "NMEA", "ntp1.example.net", "ntp2.example.org", "ntp4.example.net",
  "time-a.long-hostname.example.org", "b.ntp.example.com", "clock.example.edu", "ntp3.example.com",
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

test("Colophon: links to the repository and its issues", async ({ page }) => {
  await page.goto("/");
  const colophon = page.locator("#colophon");
  await expect(colophon.locator("a", { hasText: "github" })).toHaveAttribute("href", /github\.com\/[^/]+\/chronymon$/);
  await expect(colophon.locator("a", { hasText: "issues" })).toHaveAttribute("href", /github\.com\/[^/]+\/chronymon\/issues$/);
});

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

test("Live: chronyd's log fills before the daemon ran", async ({ page }) => {
  await page.goto("/?last=all");
  await expect(charts(page).first()).toBeVisible();
  const over = charts(page).first().locator(".u-over");
  const box = await over.boundingBox();
  // The far left of "all" is before the daemon's first poll.
  await page.mouse.move(box.x + 4, box.y + box.height / 2);
  await expect(page.locator("#readout")).toContainText("from chronyd's log");
});

test("Live: the log fills only while the daemon was stopped, and a slow clock's line spans", async ({ page }) => {
  // The synthetic run's stop, after which chronyd updates every 64 s: a
  // row only every few buckets, its log in the buckets between.
  const events = await (await page.request.get("/api/events?limit=500")).json();
  const stop = events.find((e) => e.kind === "daemon_stop").at;
  const start = events.find((e) => e.kind === "daemon_start" && e.at > stop).at;
  await page.goto(`/?from=${stop - 3600}&to=${stop + 4 * 3600}`);
  await expect(charts(page).first()).toBeVisible();
  const drawn = await page.evaluate(() => {
    const chart = stack.charts[0];
    const x = chart.data[0];
    return chart.chronymonLines.map((l) => {
      const values = chart.data[l.series];
      return {
        breaks: values.filter((v) => v === null).length,
        at: x.filter((_, i) => values[i] != null),
      };
    });
  });
  expect(drawn).toHaveLength(2);
  const [reported, logged] = drawn;
  // The reported line breaks once, for the stop, and spans the buckets
  // between its rows; the log is drawn only while it was stopped.
  expect(reported.breaks).toBe(1);
  expect(reported.at.length).toBeGreaterThan(100);
  expect(logged.breaks).toBe(0);
  expect(logged.at.length).toBeGreaterThan(0);
  for (const t of logged.at) expect(t > stop && t < start).toBe(true);
  // But for the daemon's first row after it started, whose reference
  // time is its sample's, a little before.
  for (const t of reported.at) expect(t > stop && t < start - 60).toBe(false);
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
  await expect(rows).toHaveCount(SOURCES.length);
  for (const name of SOURCES) {
    await expect(page.locator("#sources")).toContainText(name);
  }
  await expect(charts(page).first()).toBeVisible();
  await page.locator("#sources a", { hasText: "ntp1.example.net" }).first().click();
  await expect(page).toHaveURL(/\/source\?/);
  await expect(page.locator("#heading")).toContainText("ntp1.example.net");
  await expect(charts(page).first()).toBeVisible();
});

test("Sources: a placeholder for a name not resolved yet is never shown", async ({ page }) => {
  await page.goto("/sources?last=all");
  await expect(page.locator("#sources tbody tr")).toHaveCount(SOURCES.length);
  await expect(page.locator("#sources")).not.toContainText("00000001");
  await page.goto("/journal?last=all");
  await expect(page.locator("#events")).toContainText("ntp3.example.com");
  await expect(page.locator("#events")).not.toContainText("00000001");
});

test("Sources: a source with no sample shows none, not its last", async ({ page }) => {
  await page.goto("/sources?last=86400");
  const nmea = page.locator("#sources tbody tr", { hasText: "NMEA" });
  // State, name, address, mode, stratum, poll, reach, last offset.
  const cells = nmea.locator("td");
  await expect(cells.nth(0)).toHaveText("?");
  await expect(cells.nth(6)).toHaveText("0");
  await expect(cells.nth(7)).toHaveText(/^-- /);
});

test("Sources: a removed source is last, dimmed, its history kept", async ({ page }) => {
  await page.goto("/sources?last=86400");
  const rows = page.locator("#sources tbody tr");
  await expect(rows.last()).toContainText("ntp3.example.com");
  await expect(rows.last()).toHaveClass("removed");
  await expect(page.locator("#sources tbody tr.removed")).toHaveCount(1);
  await rows.last().locator("a").click();
  await expect(page.locator("#heading")).toContainText("ntp3.example.com");
  await expect(page.locator("#heading")).toContainText("removed");
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

// The synthetic sources chronyd has now, all the kiosk lists: not the
// removed ntp3.
const KIOSK_SOURCES = SOURCES.length - 1;
// Those the offset overlay draws: not the noselect NMEA.
const KIOSK_CHARTED = KIOSK_SOURCES - 1;

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
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(KIOSK_SOURCES);
  await expect(page.locator("#kiosk-source-rows")).toContainText("ntp1.example.net");
  await expect(page.locator("#kiosk-source-rows")).toContainText("NMEA");
  await expect(page.locator("#kiosk-source-rows")).not.toContainText("ntp3.example.com");
  await expect(page.locator("#kiosk-event-rows")).toContainText("clock stepped");
  await expect(page.locator("#kiosk-time")).toHaveText(/^\d\d:\d\d:\d\d$/);
  await expect(charts(page)).toHaveCount(4);
  await expect(charts(page).last().locator(".u-title")).toContainText("noselect ones left out");
  expect(await page.evaluate(() => stack.charts.at(-1).series.length - 1)).toBe(KIOSK_CHARTED);
  await expect(page.locator("nav")).toHaveCount(0);
  await expect(page.locator("#colophon")).toHaveCount(0);
  expect(await fits(page)).toBe(true);
});

// What in the kiosk overlaps or overflows, each in words: its panels
// must lie inside the body's padding and clear of one another; each
// list's rows inside its panel; the charts inside theirs; and the
// readings, names and titles not cut short.
const kioskFaults = (page) => page.evaluate(() => {
  const faults = [];
  // A little for subpixel layout.
  const SLACK = 0.5;
  const shown = (el) => el.getClientRects().length > 0;
  const label = (el) => el.id ? `#${el.id}` : `${el.tagName.toLowerCase()} "${el.textContent.slice(0, 40)}"`;
  const inner = (el) => {
    const b = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    const edge = (side) => parseFloat(s[`border${side}Width`]) + parseFloat(s[`padding${side}`]);
    return { left: b.left + edge("Left"), top: b.top + edge("Top"),
      right: b.right - edge("Right"), bottom: b.bottom - edge("Bottom") };
  };
  const inside = (a, b) => a.left >= b.left - SLACK && a.top >= b.top - SLACK &&
    a.right <= b.right + SLACK && a.bottom <= b.bottom + SLACK;
  const apart = (a, b) => a.right <= b.left + SLACK || b.right <= a.left + SLACK ||
    a.bottom <= b.top + SLACK || b.bottom <= a.top + SLACK;
  const screen = inner(document.body);
  const panels = [...document.querySelectorAll("#kiosk-state > *, #kiosk-history > *")].filter(shown);
  panels.forEach((a, n) => {
    if (!inside(a.getBoundingClientRect(), screen)) faults.push(`${label(a)} off the screen`);
    for (const b of panels.slice(n + 1)) {
      if (!apart(a.getBoundingClientRect(), b.getBoundingClientRect())) faults.push(`${label(a)} overlaps ${label(b)}`);
    }
  });
  for (const id of ["kiosk-sources", "kiosk-events"]) {
    const room = inner(document.getElementById(id));
    for (const row of document.querySelectorAll(`#${id} > div:not(.measure) tr`)) {
      if (!inside(row.getBoundingClientRect(), room)) faults.push(`${label(row)} outside #${id}`);
    }
  }
  const room = inner(document.getElementById("kiosk-stack"));
  for (const canvas of document.querySelectorAll("#charts canvas")) {
    if (!inside(canvas.getBoundingClientRect(), room)) faults.push("a chart outside #kiosk-stack");
  }
  const texts = "#kiosk-head, #kiosk-sync > *, .tile .v, .tile .k, #kiosk-source-rows td, #kiosk-event-rows td, .u-title";
  for (const el of document.querySelectorAll(texts)) {
    if (el.scrollWidth > el.clientWidth + SLACK) faults.push(`${label(el)} cut short`);
    // Wrapped: two of its pieces of text, on lines one above the other.
    const range = document.createRange();
    range.selectNodeContents(el);
    const pieces = [...range.getClientRects()].filter((b) => b.width > 0);
    if (pieces.some((a) => pieces.some((b) => a.bottom <= b.top + SLACK))) faults.push(`${label(el)} wrapped`);
  }
  return faults;
});

// A display's size and the address it is shown: full size, and a
// smaller screen that overscans.
const KIOSK_SCREENS = [
  { size: KIOSK_VIEWPORT, address: "/kiosk?last=86400" },
  { size: { width: 1280, height: 720 }, address: "/kiosk?last=86400&inset=3,4" },
];

for (const { size, address } of KIOSK_SCREENS) {
  test(`Kiosk: at ${size.width}x${size.height}, ${address}, nothing overlaps or overflows`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(address);
    await expect(charts(page)).toHaveCount(4);
    await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(KIOSK_SOURCES);
    await expect(page.locator("#kiosk-event-rows")).toContainText("clock stepped");
    expect(await kioskFaults(page)).toEqual([]);
  });
}

test("Kiosk: more sources than fit, the last row says how many more", async ({ page }) => {
  // Sources added to the server's, more than the panel holds.
  const extra = 12;
  await page.route("**/api/sources", async (route) => {
    const response = await route.fetch();
    const listed = await response.json();
    const copy = listed.find((s) => s.name === "ntp1.example.net");
    const added = Array.from({ length: extra }, (_, n) =>
      ({ ...copy, id: 1000 + n, name: `extra${n}.example.net`, address: `192.0.2.${100 + n}` }));
    return route.fulfill({ response, json: [...listed, ...added] });
  });
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk?last=86400");
  await expect(charts(page)).toHaveCount(4);
  const rows = page.locator("#kiosk-source-rows tr");
  await expect(rows.last()).toHaveText(/^and \d+ more$/);
  const more = Number((await rows.last().textContent()).match(/\d+/)[0]);
  expect(await rows.count() - 1 + more).toBe(KIOSK_SOURCES + extra);
  expect(await kioskFaults(page)).toEqual([]);
});

test.describe("Kiosk: the clock", () => {
  test.use({ timezoneId: "America/Los_Angeles" });

  test("the host's time in UTC, counted on, then stopped once no answer comes", async ({ page }) => {
    // The host's time the server is made to answer with, in seconds: an
    // evening in Los Angeles, the next day in UTC.
    const host = Date.parse("2026-01-02T03:04:05Z") / 1000;
    const started = Date.now();
    // How long answers are held back, once they are, in ms: longer than
    // the clock counts on for.
    const held = 6000;
    let holding = false;
    await page.route("**/api/status", async (route) => {
      if (holding) await new Promise((done) => setTimeout(done, held));
      const response = await route.fetch();
      const status = await response.json();
      return route.fulfill({ response, json: { ...status, now: host + (Date.now() - started) / 1000 } });
    });
    await page.setViewportSize(KIOSK_VIEWPORT);
    await page.goto("/kiosk?last=86400");
    const time = page.locator("#kiosk-time");
    await expect(time).toHaveText(/^03:04:\d\d$/);
    await expect(page.locator("#kiosk-date")).toHaveText(/^2026-01-02 · 19:04:\d\d PST 2026-01-01$/);
    const first = await time.textContent();
    await expect(time).not.toHaveText(first);
    await expect(page.locator("#kiosk-age")).toBeHidden();
    holding = true;
    await expect(page.locator("#kiosk-age")).toHaveText(/^as of \d+ s ago$/, { timeout: held });
    await expect(page.locator("#kiosk-clock")).toHaveClass(/dim/);
    const stopped = await time.textContent();
    await page.waitForTimeout(1500);
    await expect(time).toHaveText(stopped);
  });
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
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(KIOSK_SOURCES);
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
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(KIOSK_SOURCES);
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
  await expect(page.locator("#kiosk-source-rows tr")).toHaveCount(KIOSK_SOURCES);
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
