import { defineConfig } from '@playwright/test';

// explorer2 (dist-remote/) served by its own Worker config, talking to a
// local `zega start --token-file` behind a stand-in for the Fly Machine.
export default defineConfig({
  testDir: './remote-tests',
  workers: 1,
  retries: 0,
  timeout: 60_000,
  reporter: 'list',
  use: {
    baseURL: 'http://127.0.0.1:8788',
    browserName: 'chromium',
    headless: true,
    viewport: { width: 1440, height: 1000 },
  },
  webServer: {
    command: 'npm run dev:remote -- --port 8788',
    url: 'http://127.0.0.1:8788',
    reuseExistingServer: false,
    timeout: 60_000,
    env: { WRANGLER_SEND_METRICS: 'false' },
  },
});
