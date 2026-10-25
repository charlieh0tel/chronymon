// Browser tests for chronymon-web's pages.
//
// The servers are tools/with-web's: the real chronymon-web, one serving
// a synthetic database and one a path with none.  Started there rather
// than here so the tests can run in Playwright's container while the
// servers run on the host they were built for.

import { defineConfig } from "@playwright/test";

export const FULL = process.env.CHRONYMON_WEB_FULL ?? "http://127.0.0.1:9983";
export const MISSING = process.env.CHRONYMON_WEB_MISSING ?? "http://127.0.0.1:9984";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  fullyParallel: true,
  timeout: 60000,
  use: { baseURL: FULL },
});
