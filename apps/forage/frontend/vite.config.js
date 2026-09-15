import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [
    svelte({
      compilerOptions: {
        customElement: true,
      },
    }),
  ],
  build: {
    sourcemap: true,
    // Safe since Tailwind stopped scanning this bundle for class names and
    // reads frontend/src directly — see static/css/input.css. Minifying it
    // used to mangle the very string literals the scanner depended on.
    minify: true,
    lib: {
      entry: "src/main.js",
      formats: ["iife"],
      name: "ForageComponents",
      fileName: () => "forage-components.js",
    },
    outDir: "../static/js/components",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
      },
    },
  },
});
