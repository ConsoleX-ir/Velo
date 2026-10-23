import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    strictPort: true,
    host: "127.0.0.1",
    port: 1420,
    watch: { ignored: ["**/src-tauri/target/**"] },
  },
  test: { environment: "jsdom", include: ["src/**/*.test.{ts,tsx}"] },
});
