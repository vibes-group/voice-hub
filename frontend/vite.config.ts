// Output ./dist is served by the backend (APP_WEB_DIR) and bundled by Tauri
// (frontendDist). The backend auth gate lets /assets/* and /vendor/* through,
// so the login page's hashed bundles load without stable filenames.

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "path";

export default defineConfig({
  plugins: [react(), tailwindcss()],

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
