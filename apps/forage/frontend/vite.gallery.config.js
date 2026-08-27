import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Separate from vite.config.js: that one builds a custom-element library bundle
// for the server to serve. This one serves a plain page for screenshots.

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

export default defineConfig({
  root: "gallery",
  plugins: [svelte({ compilerOptions: { customElement: true } }), stubAvatars()],
  server: { port: 5178, strictPort: true },
});
