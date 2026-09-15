# forage frontend

Svelte custom elements bundled into `static/js/components/forage-components.js`
and mounted by the MiniJinja templates.

```sh
npm install
npm run build          # bundle for the server to serve
npm test               # unit tests (vitest)
npm run gallery        # the design testbed, on :5178
npm run gallery:capture  # assert rendered states + write gallery/shots/*.png
```

## The state gallery

`npm run gallery` serves every lane state the
timeline can be in, mounted from `src/lib/fixtures.js`, with no server, no
database and no releases to wait for. It is what `gallery:capture` asserts
against, and what to open when you want to *see* a state rather than read about
it.

It mounts the **real** `<release-timeline>` — a mockup would happily show the
states we intended rather than the ones the component renders, which is the
whole thing it exists to catch — and it renders with the app's own stylesheet
(`gallery/gallery.css` imports `static/css/input.css`), so a colour checked here
is the colour that ships.

The one piece of chrome is a light/dark switch. The app themes on
`prefers-color-scheme` alone, which a page cannot toggle, so
`gallery/dark-mirror.js` reads the app's own dark block out of `input.css` and
re-serves it scoped to `[data-theme="dark"]`: one palette, no second copy to
drift, and a loud failure if the shape it parses ever changes. Without it the
dark palette the component ships is unreachable from a browser.

## Testing

Three layers, sharing one set of fixtures (`src/lib/fixtures.js`) so they cannot
drift apart:

**`npm test`** — pure logic, no browser.

- `src/lib/lane-states.js` resolves each environment *and each destination* on a
  release to a lane state. Getting it wrong is silent: the timeline still
  renders, it just says the wrong thing about whether a release is finished.
- `src/lib/lane-geometry.js` turns measured row positions into the runs that
  make up a lane — a road, what is coming, what is here, what is leaving. This
  used to live inside the component, tangled up with `getBoundingClientRect` and
  a retry loop, so the only way to find out whether a rollback drew the right
  shape was to squint at it.
- `src/lib/colors.js` classifies an environment by its stage and orders the
  lanes. Colour is information here — cool means the pipeline is working, warm
  means a person is needed — so an environment in the wrong bucket is a
  correctness bug, and there is a test asserting no environment can ever take a
  warm colour.

**`npm run gallery:capture`** — what the component actually renders. Mounts the
real custom element in a browser once per fixture with `fetch` and `EventSource`
stubbed, then asserts the DOM: the resolved state reached `data-lane-states`,
unfinished states are drawn moving and finished ones are not, lanes are ordered
production-first whatever order the server sent them in, every dot sits on whole
pixels centred on its bar, no approach run stops short of the hold it runs into,
and an awaiting dot says so in words. It also clicks a lane open and checks it
fans into one strand per destination, with the failed placement's strand — and
only that strand — drawing a failure. Interaction is the one part neither layer
above can reach.

It runs Chromium by default, and `GALLERY_BROWSER=firefox npm run
gallery:capture` (or `webkit`) runs the same assertions in another engine. That
is not paranoia: a dot whose size and strand disagreed in parity inset by half a
pixel, Chrome and Firefox rounded it differently, and the result was a visible
shift in Firefox and nothing at all in Chrome. A single-engine suite could not
see it, and did not.

**`cargo test -p forage-server env_lane_color`** — the server's copy of the
palette, which feeds the timeline JSON. Kept in step with `src/lib/colors.js` by
tests on both sides.

Every assertion above was confirmed non-vacuous by breaking the thing it guards
and watching it fail. A suite that has never failed has not been shown to test
anything.

Both JS layers are wired in:

```sh
mise run test        # cargo test --workspace + frontend unit tests
mise run test:all    # the above + the rendered-state suite
```

`test` deliberately excludes the browser layer so the common case stays fast and
needs no browser download; `test:all` includes it. CI runs both on every push,
and uploads the state screenshots as a build artifact — a diff in words tells you
a state changed, the screenshots tell you what it now looks like.

## Design notes

[`design/RELEASE-SWIMLANE.md`](../../../design/RELEASE-SWIMLANE.md) covers what
the swim lane means and why it looks the way it does: the segments, the colour
rules, how environments fan out into destinations, and what each animation is
for. Read it before changing the palette or adding a lane state.
