// Browser tests for chronymon-web's pages.
//
// The servers are tools/with-web's: the real chronymon-web, one serving
// a synthetic database, one a path with none, and one each for the
// synthetic data ended as other kinds of host.  Started there rather
// than here so the tests can run in Playwright's container while the
// servers run on the host they were built for.

import { defineConfig } from "@playwright/test";

export const FULL = process.env.CHRONYMON_WEB_FULL ?? "http://127.0.0.1:9983";
export const MISSING = process.env.CHRONYMON_WEB_MISSING ?? "http://127.0.0.1:9984";
// The synthetic data ended as other kinds of host (examples/synthetic.rs).
export const NTP_HOST = process.env.CHRONYMON_WEB_NTP ?? "http://127.0.0.1:9985";
export const GPSD_STOPPED_HOST = process.env.CHRONYMON_WEB_GPSD_STOPPED ?? "http://127.0.0.1:9986";
export const BACKUPS_HOST = process.env.CHRONYMON_WEB_BACKUPS ?? "http://127.0.0.1:9987";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  fullyParallel: true,
  timeout: 60000,
  use: { baseURL: FULL },
});
