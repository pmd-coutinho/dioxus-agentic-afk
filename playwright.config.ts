import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 30_000,
  expect: {
    timeout: 10_000,
  },
  webServer: {
    command:
      'cargo run -p agentic-afk-control-plane-server --bin agentic-afk -- serve --seed-dev',
    url: 'http://127.0.0.1:3637/health',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    env: {
      AGENTIC_AFK_BIND_ADDRESS: '127.0.0.1:3637',
      AGENTIC_AFK_DATABASE_URL: 'sqlite://target/agentic-afk-playwright.db',
    },
  },
  use: {
    baseURL: 'http://127.0.0.1:3637',
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
