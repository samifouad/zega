import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './cli-tests', workers: 1, retries: 0, timeout: 60_000,
  reporter: 'list', use: { browserName: 'chromium', headless: true, viewport: { width: 1440, height: 1000 } },
});
