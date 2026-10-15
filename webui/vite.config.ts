import { defineConfig } from "vite";

const engine = "http://127.0.0.1:8080";

export default defineConfig({
  server: {
    host: "127.0.0.1",
    port: 5173,
    proxy: {
      "/api": engine,
      "/auth": engine,
      "/healthz": engine,
    },
  },
});
