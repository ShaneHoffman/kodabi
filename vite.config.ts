/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // Vitest shares this config so tests compile through the same React plugin
  // the app builds with. jsdom gives the DOM the component tests render into;
  // the setup file registers jest-dom matchers and RTL cleanup. The build and
  // server blocks below are build/serve-only and ignored here.
  test: {
    environment: "jsdom",
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },

  // Two HTML entries: the main app and the standalone quick-capture window
  // (its own Tauri webview). Plain strings resolve against the Vite root, which
  // avoids `__dirname`-in-ESM; Vite serves each as a real file in dev and emits
  // `dist/capture.html` for the packaged app.
  build: {
    rollupOptions: {
      input: {
        main: "index.html",
        capture: "capture.html",
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
