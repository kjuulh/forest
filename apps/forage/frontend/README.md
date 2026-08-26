# forage frontend

Svelte custom elements bundled into `static/js/components/forage-components.js`
and mounted by the MiniJinja templates.

```sh
npm install
npm run build          # bundle for the server to serve
npm test               # unit tests (vitest)
npm run gallery        # serve the lane-state gallery on :5178
npm run gallery:capture  # assert rendered states + write gallery/shots/*.png
```

## Testing

Two layers, sharing one set of fixtures (`src/lib/fixtures.js`) so they cannot
drift apart:

**`npm test`** — pure logic. `src/lib/lane-states.js` resolves each environment
on a release to a lane state, and getting it wrong is silent: the timeline still
renders, it just says the wrong thing about whether a release is finished. Kept
out of the `.svelte` file precisely so it can be tested without a browser.

**`npm run gallery:capture`** — what the component actually renders. Mounts the
real custom element in Chromium once per fixture with `fetch` and `EventSource`
stubbed, then asserts the DOM: the resolved state reached `data-lane-states`,
unfinished states animate and finished ones do not, and an awaiting dot says so
in words. The unit tests cannot cover this, and it is where the bug in
[spec 014](../specs/features/014-release-lane-states.md) actually lived — the
logic said `pending`, and `pending` happened to look finished.

It starts its own vite server, so it is one command — requiring a second
terminal is the kind of friction that stops a suite from being run.

Both suites were confirmed non-vacuous by reverting the fix and watching them
fail — 8 unit tests and 9 rendered assertions. A suite that has never failed has
not been shown to test anything.

Both are wired in:

```sh
mise run test        # cargo test --workspace + frontend unit tests
mise run test:all    # the above + the rendered-state suite
```

`test` deliberately excludes the browser layer so the common case stays fast and
needs no browser download; `test:all` includes it. CI runs both on every push,
and uploads the state screenshots as a build artifact — a diff in words tells you
a state changed, the screenshots tell you what it now looks like.
