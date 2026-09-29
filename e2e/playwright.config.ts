import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  retries: 0,
  // Specs share the webServer and live-reload rewrites its fixture
  workers: process.env.CI ? 1 : undefined,
  use: {
    baseURL: "http://127.0.0.1:3456",
  },
  webServer: {
    command:
      "cargo run -- --no-open --port 3456 fixtures/test.md",
    url: "http://127.0.0.1:3456/health",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    cwd: __dirname,
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
    // Selection and review behaviour diverges in WebKit, so those specs run there too
    {
      name: "webkit",
      use: { browserName: "webkit" },
      testMatch: /(selection|patch|review\/|csp).*\.spec\.ts/,
    },
    ...(process.env.BIRTA_FF === "1"
      ? [{ name: "firefox", use: { browserName: "firefox" as const } }]
      : []),
  ],
});
