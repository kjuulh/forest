import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Separate from vite.config.js: that one builds a custom-element library bundle
// for the server to serve. This one serves a plain page for screenshots.
export default defineConfig({
  root: "gallery",
  plugins: [svelte({ compilerOptions: { customElement: true } })],
  server: { port: 5178, strictPort: true },
});
