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

test("API: a request too large is refused, saying why", async ({ page }) => {
  const twice = await page.request.get(`/api/history?columns=${Array(600).fill("skew").join(",")}`);
  expect(twice.status()).toBe(400);
  expect((await twice.json()).error).toBe("skew is in columns= twice");
  const points = await page.request.get("/api/overlay?table=ntp_sample&column=offset&points=5001");
  expect(points.status()).toBe(400);
  expect((await points.json()).error).toMatch(/^points=5001 is more than 5000$/);
});

// Each chart's time axis, `[min, max]`, on the page now.
const timeAxes = (page) => page.evaluate(() => stack.charts.map((c) => [c.scales.x.min, c.scales.x.max]));

// The clock's last offsets as `/api/history` answers them, each bucket
// that has one passed through `change(seconds, n)`, `n` its count among
// them, into mean, lowest and highest alike.
const routeLastOffset = (page, change) =>
  page.route("**/api/history?*", async (route) => {
    const response = await route.fetch();
    const got = await response.json();
    for (const plot of got.plots.filter((p) => p.table === "tracking" && p.column === "last_offset")) {
      let n = 0;
      plot.mean.forEach((v, i) => {
        if (v == null) return;
        const changed = change(v, n++);
        if (changed === v) return;
        plot.mean[i] = plot.min[i] = plot.max[i] = changed;
      });
    }
    return route.fulfill({ response, json: got });
  });

// A step of seconds beside offsets of microseconds: an asinh axis.
const STEP = -20;

// Each chart's y tick labels too near the next to clear it, by uPlot's
// position for each and the axes' type size, as `[chart, a, b]`.
const crowdedTicks = (page) => page.evaluate(() => {
  const type = Number((stack.font ?? "12px").match(/\d+/)[0]);
  return stack.charts.flatMap((c, n) => {
    const at = c.axes[1].splits?.(c, 1, c.scales.y.min, c.scales.y.max) ?? [];
    const ticks = at.map((v) => [v, c.valToPos(v, "y")]).sort((a, b) => a[1] - b[1]);
    return ticks.slice(1).flatMap(([v, p], i) => (p - ticks[i][1] < type ? [[n, ticks[i][0], v]] : []));
  });
});

// The NMEA refclock's ID: its samples stop hours before now.
const nmeaId = async (page) =>
  (await (await page.request.get("/api/sources")).json()).find((s) => s.name === "NMEA").id;

test("Every stack: its charts share one time axis, the range's, to its end", async ({ page }) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  const nmea = await nmeaId(page);
  for (const path of [
    "/?last=86400",
    "/sources?last=86400",
    `/source?source=${nmea}&last=86400`,
    `/source?source=${nmea}&last=all`,
    "/refclocks?last=86400",
    "/temperature?last=86400",
    "/kiosk?last=86400",
  ]) {
    await page.goto(path);
    await expect(charts(page).first()).toBeVisible();
    await page.waitForFunction(() => !running);
    const axes = await timeAxes(page);
    expect(axes.length, path).toBeGreaterThan(1);
    for (const axis of axes) expect(axis, path).toEqual(axes[0]);
    // Up to now, though NMEA's samples, and so the last of its rows in
    // every table, stopped hours before.
    expect(axes[0][1], path).toBeGreaterThan(Date.now() / 1000 - 120);
    if (path.endsWith("86400")) expect(axes[0][1] - axes[0][0], path).toBeCloseTo(86400, -1);
  }
});

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

// The clock's offsets made this many times smaller, a fraction of a
// nanosecond: beside a step of seconds, its asinh axis's threshold sits
// a few pixels from zero.
const QUIET = 1e-4;

for (const columns of ["last_offset", "last_offset,frequency,skew,rms_offset,root_delay,root_dispersion"]) {
  test(`Live: an asinh axis's tick labels clear one another, zero kept, ${columns}`, async ({ page }) => {
    await routeLastOffset(page, (v, n) => (n === 300 ? STEP : v * QUIET));
    await page.goto(`/?last=21600&columns=${columns}`);
    await expect(charts(page).first()).toBeVisible();
    await page.waitForFunction(() => !running);
    const axis = await page.evaluate(() => {
      const c = stack.charts[0];
      return { distr: c.scales.y.distr, min: c.scales.y.min, ticks: c.axes[1].splits() };
    });
    expect(axis.distr).toBe(4);
    expect(axis.min).toBeLessThanOrEqual(STEP * 1e6);
    expect(axis.ticks).toContain(0);
    expect(await crowdedTicks(page)).toEqual([]);
  });
}

test("Live: a column unticked leaves its chart", async ({ page }) => {
  await page.goto("/?last=86400");
  await expect(charts(page).first()).toBeVisible();
  const before = await charts(page).count();
  await page.locator("#columns input[data-col]:checked").first().uncheck();
  await expect(charts(page)).toHaveCount(before - 1);
});

// The readout's value for `name`, as a number, or null for "--".
const readoutValue = async (page, name) => {
  const text = await page.locator("#readout > span", { hasText: name }).locator(".v").textContent();
  return text === "--" ? null : Number(text.replace(/,/g, ""));
};

// The last `/api/history` the page read, as an address.
const lastHistory = (page) => {
  const read = { url: null };
  page.on("request", (r) => {
    if (r.url().includes("/api/history?")) read.url = new URL(r.url());
  });
  return read;
};

// The bucketed `column` of `table` alone, on the grid of `read`, the
// page's history read.
async function onGridOf(page, read, table, column) {
  const q = new URLSearchParams(read.searchParams);
  for (const k of ["fill", "fill_columns"]) q.delete(k);
  q.set("table", table);
  q.set("columns", column);
  const got = await (await page.request.get(`/api/history?${q}`)).json();
  return { at: got.at, mean: got.plots[0].mean };
}

const FREQUENCY = "Frequency, reported";
const LOGGED_FREQUENCY = "Frequency, from chronyd's log";

test("Live: chronyd's log fills before the daemon ran, with the log's values", async ({ page }) => {
  const read = lastHistory(page);
  await page.goto("/?last=all&columns=frequency");
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  const over = charts(page).first().locator(".u-over");
  const box = await over.boundingBox();
  // The far left of "all" is before the daemon's first poll.
  await page.mouse.move(box.x + 4, box.y + box.height / 2);
  await expect(page.locator("#readout")).toContainText(LOGGED_FREQUENCY);
  const at = await page.evaluate(() => stack.charts[0].data[0][stack.charts[0].cursor.idx]);
  const log = await onGridOf(page, read.url, "log_tracking", "frequency");
  const expected = log.mean[log.at.indexOf(at)];
  expect(expected).toBeLessThan(-10);
  expect(await readoutValue(page, LOGGED_FREQUENCY)).toBeCloseTo(expected, 3);
  expect(await readoutValue(page, FREQUENCY)).toBeNull();
});

test("Live: where the daemon recorded, its rows are drawn, not the log's", async ({ page }) => {
  // After the synthetic run's stop and start, chronyd's log goes on
  // beside the daemon's rows.
  const events = await (await page.request.get("/api/events?limit=500")).json();
  const stop = events.find((e) => e.kind === "daemon_stop").at;
  const start = events.find((e) => e.kind === "daemon_start" && e.at > stop).at;
  const read = lastHistory(page);
  await page.goto(`/?from=${Math.ceil(start + 600)}&to=${Math.ceil(start + 4200)}&columns=frequency`);
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  const log = await onGridOf(page, read.url, "log_tracking", "frequency");
  expect(log.mean.filter((v) => v != null).length).toBeGreaterThan(10);
  const box = await charts(page).first().locator(".u-over").boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await expect(page.locator("#readout")).not.toContainText(LOGGED_FREQUENCY);
  expect(await readoutValue(page, "Frequency")).toBeLessThan(-10);
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

// The synthetic run's time not synchronized to a source: when it lost
// sync, went on to its local reference and was synchronized again, by
// its leap and reference changes.
async function notSynchronized(page) {
  const events = await (await page.request.get("/api/events?limit=500")).json();
  const at = (kind, test) => events.find((e) => e.kind === kind && test(e.detail)).at;
  return {
    lost: at("leap_change", (d) => d.endsWith(" to Unsynchronized")),
    local: at("reference_change", (d) => d.endsWith(" to local")),
    regained: at("reference_change", (d) => d.startsWith("local to ")),
  };
}

const NOT_SYNCHRONIZED = "chronyd not synchronized";
const ON_LOCAL = "chronyd on its local reference";

test("Live: chronyd unsynchronized or on its local reference is a gap, shaded and named", async ({ page }) => {
  const { lost, local, regained } = await notSynchronized(page);
  const read = lastHistory(page);
  await page.goto(`/?from=${Math.floor(lost - 1800)}&to=${Math.ceil(regained + 1800)}&columns=frequency`);
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  const got = await (await page.request.get(read.url.toString())).json();
  expect(got.unsynchronized).toEqual([[lost, local]]);
  expect(got.local).toEqual([[local, regained]]);
  const drawn = await page.evaluate(([from, local, to]) => {
    const chart = stack.charts[0];
    const values = chart.data[1];
    return {
      bands: stack.read.bands.map((b) => [b.from, b.to]),
      breaks: values.filter((v) => v === null).length,
      // The line's values in the spans, but the update ending them, a
      // little before the poll that saw it.
      within: chart.data[0].filter((t, i) => values[i] != null && t > from && t < to - 60).length,
      unsynchronized: chart.valToPos((from + local) / 2, "x"),
      // In the local reference's span, and as far as can be from an
      // update, which the cursor would otherwise read.
      middle: chart.valToPos((from + to) / 2, "x"),
    };
  }, [lost, local, regained]);
  expect(drawn.bands).toEqual([[lost, local], [local, regained]]);
  expect(drawn.breaks).toBe(1);
  expect(drawn.within).toBe(0);
  const box = await charts(page).first().locator(".u-over").boundingBox();
  for (const [x, text] of [[drawn.unsynchronized, NOT_SYNCHRONIZED], [drawn.middle, ON_LOCAL]]) {
    await page.mouse.move(box.x + x, box.y + box.height / 2);
    await expect(page.locator("#readout")).toContainText(text);
    await expect(page.locator("#note-tip")).toHaveText(text);
  }
  expect(await readoutValue(page, "Frequency")).toBeNull();
  // A range that does not reach them has none.
  await page.goto(`/?from=${Math.ceil(regained + 600)}&to=${Math.ceil(regained + 3600)}&columns=frequency`);
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  expect(await page.evaluate(() => stack.read.bands)).toEqual([]);
});

test("Kiosk: chronyd unsynchronized or on its local reference is shaded on its clock's charts only", async ({ page }) => {
  const { lost, local, regained } = await notSynchronized(page);
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto(`/kiosk?from=${Math.floor(lost - 1800)}&to=${Math.ceil(regained + 1800)}`);
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  // Each chart's title and the bands drawn behind it.
  const shaded = await page.evaluate(() => stack.charts.map((c) => [
    c.root.querySelector(".u-title").textContent.split(",")[0],
    c.chronymonBands().map((b) => [b.from, b.to]),
  ]));
  const both = [[lost, local], [local, regained]];
  // Not the sources' offsets, which go on while chronyd is not
  // synchronized to one.
  expect(shaded).toEqual([["Last offset", both], ["Frequency", both], ["Skew", both], ["Offset", []]]);
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

// Each chart of the Sources page: whether it draws `name`'s line, and
// shows it, its title's button for it, and its y axis's span.
const lineCharts = (page, name) => page.evaluate((name) => stack.charts.map((c) => {
  const line = c.chronymonLines.find((l) => l.name === name);
  const button = c.root.querySelector(`.u-title button[data-line="${CSS.escape(name)}"]`);
  return {
    title: c.root.querySelector(".u-title").textContent,
    drawn: Boolean(line),
    shown: line ? c.series[line.series].show : null,
    pressed: button?.getAttribute("aria-pressed") ?? null,
    span: c.scales.y.max - c.scales.y.min,
  };
}), name);

// A fixed day, not the last one: a window that slides with the clock
// reads other buckets at its edges on each redraw, and an axis a test
// expects unchanged can refit.
const fixedDay = () => {
  const to = Math.floor(Date.now() / 1000) - 60;
  return `from=${to - 86400}&to=${to}`;
};

// The Sources page at `path`, drawn.
const drawnSources = async (page, path) => {
  await page.goto(path);
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
};

test("API: a refclock a PPS is locked to says which", async ({ page }) => {
  const sources = await (await page.request.get("/api/sources")).json();
  const lockedBy = Object.fromEntries(sources.map((s) => [s.name, s.locked_by]));
  expect(lockedBy.NMEA).toBe("PPS");
  expect(lockedBy.PPS).toBeNull();
  expect(lockedBy["ntp1.example.net"]).toBeNull();
});

test("Sources: the table marks a refclock a PPS is locked to, quietly", async ({ page }) => {
  await page.goto("/sources?last=3600");
  const row = (name) => page.locator("#sources tbody tr", { has: page.locator("a", { hasText: new RegExp(`^${name}$`) }) });
  const paired = row("NMEA").locator(".paired");
  await expect(paired).toHaveText("PPS locked to it");
  await expect(paired).toHaveClass(/\bmuted\b/);
  await expect(row("PPS").locator(".paired")).toHaveCount(0);
  await expect(row("ntp1.example.net").locator(".paired")).toHaveCount(0);
});

test("Sources: a refclock a PPS is locked to starts hidden, shown by its name, kept in the address", async ({ page }) => {
  await drawnSources(page, `/sources?${fixedDay()}`);
  const before = await lineCharts(page, "NMEA");
  const withNmea = before.filter((c) => c.drawn);
  // The offset, and the regression's frequency, skew and deviation.
  expect(withNmea.length).toBeGreaterThan(3);
  expect(withNmea[0].title).toMatch(/^Offset/);
  for (const c of withNmea) expect([c.shown, c.pressed], c.title).toEqual([false, "false"]);
  await expect(page.locator("#readout")).not.toContainText("NMEA");
  await page.locator('#charts .u-title button[data-line="NMEA"]').first().click();
  await expect(page).toHaveURL(/[?&]show=NMEA(&|$)/);
  const shown = async () => {
    const after = await lineCharts(page, "NMEA");
    for (const c of after.filter((c) => c.drawn)) expect([c.shown, c.pressed], c.title).toEqual([true, "true"]);
    // Each axis NMEA is on is widened by its wander, and the rest left
    // as they were.
    after.forEach((c, n) => {
      if (before[n].drawn) expect(c.span, c.title).toBeGreaterThan(before[n].span);
      else expect(c.span, c.title).toBe(before[n].span);
    });
  };
  await shown();
  await page.reload();
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  await shown();
  // By keyboard, and the button keeps the focus.
  const button = page.locator('#charts .u-title button[data-line="NMEA"]').last();
  await button.focus();
  await page.keyboard.press("Enter");
  await expect(page).not.toHaveURL(/show=/);
  expect(await page.evaluate(() => document.activeElement?.dataset.line)).toBe("NMEA");
  const hidden = await lineCharts(page, "NMEA");
  hidden.forEach((c, n) => expect(c, c.title).toEqual(before[n]));
});

test("Sources: any other source's name in a title hides its line on every chart", async ({ page }) => {
  await drawnSources(page, `/sources?${fixedDay()}`);
  const before = await lineCharts(page, "PPS");
  expect(before.filter((c) => c.drawn).length).toBeGreaterThan(3);
  for (const c of before.filter((c) => c.drawn)) expect([c.shown, c.pressed], c.title).toEqual([true, "true"]);
  await page.locator('#charts .u-title button[data-line="PPS"]').first().click();
  await expect(page).toHaveURL(/[?&]hide=PPS(&|$)/);
  await drawnSources(page, page.url());
  for (const c of (await lineCharts(page, "PPS")).filter((c) => c.drawn)) {
    expect([c.shown, c.pressed], c.title).toEqual([false, "false"]);
  }
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
  const figures = page.locator("#figures");
  await expect(figures).toContainText(/, r = -?\d\.\d{3}, r² = \d\.\d{3}/);
  // r² is r squared, to the figures shown.
  const [, r, squared] = (await figures.textContent()).match(/r = (-?\d\.\d{3}), r² = (\d\.\d{3})/);
  expect(Number(squared)).toBeCloseTo(Number(r) ** 2, 2);
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
    "leap status changed",
  ]) {
    await expect(events).toContainText(what);
  }
});

// A kiosk's display: landscape, as a television is.
const KIOSK_VIEWPORT = { width: 1920, height: 1080 };

// The synthetic sources chronyd has now, all the kiosk lists: not the
// removed ntp3.
const KIOSK_SOURCES = SOURCES.length - 1;
// Those the offset overlay draws: not NMEA, which the PPS is locked to.
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
  await expect(charts(page).last().locator(".u-title")).toContainText("NMEA left out, PPS locked to it");
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

// The synthetic run's step, in the clock's last offset, µs.
const SYNTHETIC_STEP = 0.18e6;

// Each chart's title, y axis and what lies off it: its range and kind,
// how many buckets it draws at each edge and their markers, and its
// lines' lowest and highest values as drawn.
const yAxes = (page) => page.evaluate(() => stack.charts.map((c) => {
  const off = c.chronymonAxis.fit.off;
  const drawn = c.data.slice(1).flat().filter((v) => v != null);
  return {
    title: c.root.querySelector(".u-title").textContent,
    min: c.scales.y.min,
    max: c.scales.y.max,
    distr: c.scales.y.distr,
    below: off?.below ?? 0,
    above: off?.above ?? 0,
    marks: off?.marks.length ?? 0,
    lowest: Math.min(...drawn),
    highest: Math.max(...drawn),
  };
}));

// The words a title ends with for the buckets drawn at its axis's edge.
const offScaleWords = (axis) => `; ${axis.below} below and ${axis.above} above the axis`;

test("Kiosk: a step past the bulk is drawn at the axis's edge, marked and counted", async ({ page }) => {
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk?last=86400");
  await expect(charts(page)).toHaveCount(4);
  await page.waitForFunction(() => !running);
  const [offset] = await yAxes(page);
  // Fitted to the microseconds of wander, the step's 0.18 s above it.
  expect(offset.max).toBeLessThan(SYNTHETIC_STEP / 100);
  expect(offset.distr).toBe(1);
  expect(offset.above).toBeGreaterThan(0);
  expect(offset.marks).toBe(offset.below + offset.above);
  expect(offset.highest).toBe(offset.max);
  expect(offset.title.endsWith(offScaleWords(offset))).toBe(true);
  expect(await kioskFaults(page)).toEqual([]);
});

test("Kiosk: nothing past the bulk, nothing clipped", async ({ page }) => {
  await page.setViewportSize(KIOSK_VIEWPORT);
  // Six quiet hours after the step.
  await page.goto("/kiosk?last=21600");
  await expect(charts(page)).toHaveCount(4);
  await page.waitForFunction(() => !running);
  for (const axis of await yAxes(page)) {
    expect(axis.marks, axis.title).toBe(0);
    expect(axis.title, axis.title).not.toContain("above the axis");
    expect(axis.lowest, axis.title).toBeGreaterThanOrEqual(axis.min);
    expect(axis.highest, axis.title).toBeLessThanOrEqual(axis.max);
  }
  await expect(page.locator("#charts .off-scale")).toHaveCount(0);
});

test("Live: the step sets the axis, nothing clipped", async ({ page }) => {
  await page.goto("/?last=86400");
  await expect(charts(page).first()).toBeVisible();
  await page.waitForFunction(() => !running);
  const axes = await yAxes(page);
  expect(axes[0].max).toBeGreaterThanOrEqual(SYNTHETIC_STEP);
  for (const axis of axes) {
    expect(axis.marks, axis.title).toBe(0);
    expect(axis.lowest, axis.title).toBeGreaterThanOrEqual(axis.min);
    expect(axis.highest, axis.title).toBeLessThanOrEqual(axis.max);
  }
  await expect(page.locator("#charts .off-scale")).toHaveCount(0);
});

// One bucket in this many of the clock's last offset made a tail of
// excursions, a decade apart up to milliseconds: past an asinh axis's
// reach, and too many to be clipped.
const TAIL_EVERY = 20;
const TAIL = [1e-4, -1e-3, 1e-2];

test("Kiosk: clipped to an asinh bulk, a step counted, its tick labels clear one another", async ({ page }) => {
  await routeLastOffset(page, (v, n) =>
    n === 300 ? STEP : n % TAIL_EVERY === 0 ? TAIL[(n / TAIL_EVERY) % TAIL.length] : v * QUIET);
  await page.setViewportSize(KIOSK_VIEWPORT);
  await page.goto("/kiosk?last=21600");
  await expect(charts(page)).toHaveCount(4);
  await page.waitForFunction(() => !running);
  const [offset] = await yAxes(page);
  expect(offset.distr).toBe(4);
  expect(offset.min).toBeGreaterThan(STEP * 1e6);
  expect(offset.max).toBeGreaterThanOrEqual(Math.max(...TAIL) * 1e6);
  expect(offset.below).toBe(1);
  expect(offset.lowest).toBe(offset.min);
  expect(offset.title.endsWith(offScaleWords(offset))).toBe(true);
  expect(await crowdedTicks(page)).toEqual([]);
});

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
    // The answers still held back are the test's, not the page's.
    await page.unrouteAll({ behavior: "ignoreErrors" });
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

test("Distribution: all is every reading, not the latest hour's", async ({ page }) => {
  // The readings the page's first figures count.
  const readings = async () => {
    const cell = page.locator("#figures table").first().locator("tr", { hasText: "readings" }).locator("td");
    return Number((await cell.textContent()).replace(/\D/g, ""));
  };
  await page.goto("/distribution?last=86400&columns=last_offset");
  await expect(page.locator("#figures table")).toHaveCount(1);
  const day = await readings();
  const asked = page.waitForRequest(/\/api\/distribution\?.*from=0/);
  await page.goto("/distribution?last=all&columns=last_offset");
  await asked;
  await expect(page.locator("#figures table")).toHaveCount(1);
  const all = await readings();
  const whole = await (await page.request.get("/api/distribution?column=last_offset&from=0")).json();
  expect(all).toBe(whole.count);
  // 48 h of the daemon's rows, against a day's.
  expect(all).toBeGreaterThan(1.5 * day);
});
