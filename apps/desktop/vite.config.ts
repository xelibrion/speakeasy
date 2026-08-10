import { defineConfig } from "vite";

export default defineConfig({
  // Tauri drives the dev server; fail loudly rather than silently picking
  // another port, since devUrl in tauri.conf.json is fixed.
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2022",
    sourcemap: true,
  },
});
