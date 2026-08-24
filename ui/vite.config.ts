import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri's own documented Vite setup: a fixed dev-server port (matching
// tauri.conf.json's build.devUrl) that fails loudly rather than silently
// picking a different one if it's taken, and ignoring the Rust source
// tree so a `cargo build` touching target/ doesn't trigger a frontend
// reload. https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
});
