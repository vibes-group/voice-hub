// Voice Hub — Vite multi-page config
//
// Build output: ./dist (Vite default). Consumed by the backend (via APP_WEB_DIR
// env / config default) and by Tauri (frontendDist in tauri.conf.json).
//
// Auth whitelist approach:
//   Vite emits hashed assets under /assets/*.js|css. The backend's auth gate
//   passes through any request whose path starts with "/assets/" or
//   "/vendor/" — both are pure static files with no sensitive content.
//   That covers all Vite-generated bundles (including the login page's
//   JS chunk) without us needing stable filenames.

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "path";

export default defineConfig({
  plugins: [react(), tailwindcss()],

  base: "/",

  server: {
    port: 5173,
    proxy: {
      "/api": "http://localhost:8080",
      "/ws": { target: "ws://localhost:8080", ws: true },
    },
  },

  build: {
    sourcemap: false,

    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        login: resolve(__dirname, "login.html"),
        connect: resolve(__dirname, "connect.html"),
      },
    },
  },
});
