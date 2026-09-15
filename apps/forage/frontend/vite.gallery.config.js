import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { darkMirrorCss } from "./gallery/dark-mirror.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const INPUT_CSS = join(HERE, "..", "static", "css", "input.css");

// Separate from vite.config.js: that one builds a custom-element library bundle
// for the server to serve. This one serves a design testbed — the real
// components, the real stylesheet, against fixtures instead of a server.

// The release cards ask the server for /avatars/<username>. There is no forage
// server behind the gallery, so stand in for one: kjuulh has a picture, anybody
// else does not — which is exactly the pair the avatar slot has to render, the
// picture and the initial it falls back to when the request 404s.
const AVATAR = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48" width="48" height="48">
  <rect width="48" height="48" fill="#0f766e"/>
  <circle cx="24" cy="18" r="8" fill="#99f6e4"/>
  <path d="M8 48c0-9 7-16 16-16s16 7 16 16z" fill="#99f6e4"/>
</svg>`;

function stubAvatars() {
  return {
    name: "gallery-stub-avatars",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const match = /^\/avatars\/([^/?]+)/.exec(req.url || "");
        if (!match) return next();
        if (decodeURIComponent(match[1]) !== "kjuulh") {
          res.statusCode = 404;
          return res.end();
        }
        res.setHeader("content-type", "image/svg+xml");
        res.end(AVATAR);
      });
    },
  };
}

// Serves the app's dark palette re-scoped to `[data-theme="dark"]`, so the
// toolbar's theme switch drives the real thing rather than a copy of it.
// See gallery/dark-mirror.js.
function darkMirror() {
  const VIRTUAL = "/@gallery/dark-mirror.css";
  return {
    name: "gallery-dark-mirror",
    configureServer(server) {
      server.watcher.add(INPUT_CSS);
      server.middlewares.use((req, res, next) => {
        if ((req.url || "").split("?")[0] !== VIRTUAL) return next();
        try {
          const css = darkMirrorCss(readFileSync(INPUT_CSS, "utf8"));
          res.setHeader("content-type", "text/css");
          res.end(css);
        } catch (err) {
          res.statusCode = 500;
          res.setHeader("content-type", "text/css");
          res.end(`/* ${err.message} */\nbody::before{content:"${err.message}";color:red}`);
        }
      });
    },
  };
}

export default defineConfig({
  root: "gallery",
  plugins: [
    tailwindcss(),
    svelte({ compilerOptions: { customElement: true } }),
    stubAvatars(),
    darkMirror(),
  ],
  server: { port: 5178, strictPort: true },
});
