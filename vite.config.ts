import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { strictPort: true, host: "127.0.0.1", port: 1420 },
  test: { environment: "jsdom", include: ["src/**/*.test.{ts,tsx}"] },
});
