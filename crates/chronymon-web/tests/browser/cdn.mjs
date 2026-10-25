// uPlot from node_modules, at the version the pages ask jsdelivr for,
// so the browser needs no network; anything else off this host fails.

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);
const UPLOT = dirname(require.resolve("uplot/package.json"));

const CDN = /^https:\/\/cdn\.jsdelivr\.net\/npm\/uplot@1\.6\.32\/dist\/(.+)$/;

export async function routeCdn(page) {
  await page.route(/^https?:\/\/(?!127\.0\.0\.1)/, async (route) => {
    const match = CDN.exec(route.request().url());
    if (!match) return route.abort();
    const body = readFileSync(join(UPLOT, "dist", match[1]));
    const contentType = match[1].endsWith(".css") ? "text/css" : "application/javascript";
    return route.fulfill({ body, contentType });
  });
}
