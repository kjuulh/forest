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

Needs the gallery served first:

```sh
npm run gallery &          # :5178
npm run gallery:capture
```

Both suites were confirmed non-vacuous by reverting the fix and watching them
fail — 8 unit tests and 9 rendered assertions. A suite that has never failed has
not been shown to test anything.

> These are not yet wired into `mise run test` (`cargo test --workspace`).
> Running them is a manual step.
