import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

// Tauri serves this over a fixed port and expects the dev server to fail loudly
// rather than silently pick another one.
export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust rebuilds are Cargo's job; watching target/ would thrash the CPU.
      ignored: ["**/target/**", "**/crates/**"],
    },
  },
  build: {
    target: "esnext",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
