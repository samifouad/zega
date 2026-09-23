import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  workers: 1,
  retries: 0,
  timeout: 60_000,
  reporter: 'list',
  use: {
    baseURL: 'http://127.0.0.1:8787',
    browserName: 'chromium',
    headless: true,
    viewport: { width: 1440, height: 1000 },
  },
  webServer: {
    command: 'npm run dev -- --port 8787',
    url: 'http://127.0.0.1:8787',
    reuseExistingServer: false,
    timeout: 60_000,
    env: { WRANGLER_SEND_METRICS: 'false' },
  },
});
