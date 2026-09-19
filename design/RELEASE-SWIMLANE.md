# Release swimlane — research and design direction

The releases view answers one question, and it should answer it from across the
room: **what is live where, and what needs a person?**

Everything below serves that. Reference material lives in
[`references/swimlane/`](references/swimlane/), captured from Uber's
[Up continuous deployment](https://www.uber.com/us/en/blog/continuous-deployment/)
write-up.

---

## What Uber's Up actually does

The four reference shots, read carefully:

**`uber-cd-02-overview.png`** — the whole surface. A narrow gutter of vertical
coloured capsules on the left, one per environment, labelled bottom-up in the
lane's own colour (`production`, `preprod`, `staging`). To its right, a single
column of commit cards. Each lane is *filled solid* from the bottom up to the
commit that environment currently holds, and *pale* above it. Small hollow pips
mark every commit row the lane passes; a filled pip marks where the environment
actually sits. Between cards, `8 hidden commits · Show commits` with a squiggle
connector in the gutter.

**`uber-cd-01-hero.png`** — the lanes in motion. The pale section above a lane's
filled head carries a column of chevrons pointing *toward* the head: the deploy
is travelling. The lane and the badge on the card agree — both carry a `▲`.

**`uber-cd-03-lane-tooltip.png`** — hover a lane and you get a card:
`Environment: production` (a link), then `Commit`, `Status`, `User`, `Started`.
The lane in this shot is amber with chevrons pointing *down*, and the card badge
reads `Rolling back in production ▼`. Direction is encoded twice, in colour and
in glyph.

**`uber-cd-04-expanded-card.png`** — an expanded commit. A stage ledger:
`Build created`, `Deployed to staging`, `Deployed to preprod`, `Soaked`,
`Waiting for time window`, then `Deploy to production` **greyed out** — the
stage that has not happened yet is still listed. Then a footer of actions:
`Skip to this commit`, `Deploy..`, `Pin`.

### The ideas worth stealing

1. **The gutter is a track diagram, not a chart.** A lane is a physical thing
   with a head position, history behind it and travel ahead of it.
2. **The future is drawn.** "Every commit can be expanded to understand not only
   exactly what has happened to it already, but also what is going to happen for
   it in the future." A pending stage is rendered, greyed, in its place in the
   order.
3. **Direction is a first-class state.** Forward and backward are different
   colours *and* different glyphs. A rollback never reads as a deploy.
4. **Noise collapses.** "Collapsed in-between commits to provide a clearer
   view." Commits that touched nothing are folded behind one line.
5. **The UI owns the truth, not the pipeline.** Manual deploys made outside the
   pipeline are folded back in, so the diagram is what is *actually* running.

### Vocabulary Up uses

`soak` / soak time · `deployment window` · `gating conditions` · `rollback` ·
`abort` · `skip waiting` · `skip to this commit` · `pin` · `downtime` ·
`hidden commits` · `previously deployed to`.

Worth adopting where forest has the same concept: forest's `wait` stage is
Uber's soak/time window, and forest's `gate` stage is Uber's gating conditions.

---

## Where forest differs: destinations

Up's diagram is one strand per environment. Forest has one more level:

```
environment            prod
  └── destination      prod/eu-north-1      ← a placement within the env
  └── destination      prod/eu-west-1
  └── destination      prod/hetzner-fsn1
```

A release does not "reach prod"; it reaches *each destination in prod*, on its
own schedule, with its own status, its own queue position and its own failure.
An environment is green only when all of its destinations are. Today the
timeline flattens this: the gutter is per-environment, and destinations appear
as an undifferentiated list inside the expanded card.

That flattening hides the state that matters most during an incident — **a
partial rollout**. `prod` showing one solid lane is a lie when two of its three
destinations have the new release and the third failed.

### The resolution: lanes that fan out

An environment lane is a **bundle**. Collapsed it is one capsule carrying the
environment's aggregate state, with the number of placements on its label —
`prod 3`. Click it and the bundle **fans out**, with an animated width
transition, into one thin strand per destination — each with its own head, its
own history, its own dots and its own hover card. Click again and it gathers
back.

The count lives on the label rather than on the bar. A first attempt drew a
hairline down the middle of a collapsed lane whose destinations disagreed, and
it read as a seam in the rendering rather than as a signal — the bar is a
picture of one thing, and splitting it says the picture is broken.

That keeps the default view as calm as Uber's while making the layer forest
actually has reachable in one click, at the exact moment it matters.

---

## Visual system

### Colour: cool is the machine working, warm is you

The current palette is a rainbow — indigo, pink, orange, yellow, violet, sky,
amber, cyan — assigned by substring match, with no relationship between a
colour and what it means. Amber means `finance`, and also means "went
backwards". That is the thing to fix.

Environments get a **cool hue sweep**, weighted so production is the heaviest
mark in the gutter:

| Stage | Light | Dark | Why |
|---|---|---|---|
| `prod` | `#2563eb` blue-600 | `#60a5fa` | Deepest, most saturated — the eye lands here first |
| `preprod` | `#7c3aed` violet-600 | `#a78bfa` | One step around the wheel, one step lighter |
| `staging` | `#c026d3` fuchsia-600 | `#e879f9` | |
| `test` | `#0891b2` cyan-600 | `#22d3ee` | Off the blue→magenta sweep entirely: not on the road to prod |
| `dev` | `#0d9488` teal-600 | `#2dd4bf` | |
| anything else | `#64748b` slate-500 | `#94a3b8` | Deterministic hue from the name, same lightness band |

The stage is read off the **last** segment of the name, so `data-prod` is
production-blue and `platform-dev` is dev-teal — today's substring scan gets
this right by accident and would get `prod-metrics` wrong.

Warm is then free to mean exactly one thing — **a person is needed, or
something went wrong** — and never anything else:

| | |
|---|---|
| `#d97706` amber-600 | awaiting approval · rolling back · went backwards |
| `#dc2626` red-600 | failed · timed out |
| `#059669` emerald-600 | a stage succeeded (the tick, not the lane) |

Dots are rings rather than shadows so that `pending` — the one state that has
not happened yet — can be dashed, and cannot be confused with `past`, which is
the same shape but did happen.

Every dot is sized from the strand it sits on and nothing paints outside that
diameter. A fixed size does not work: the head dot was once as wide as a
collapsed lane and nearly twice as wide as a fanned strand, so it bulged past
the capsule and the lane read as a lollipop rather than as a rail with a marker
on it. The head is a bullseye — a ring in the page colour punched out of the
bar — so it reads as part of the lane instead of something stuck on top.

### Motion: one moment, earned

Motion here is not decoration; a deployment surface is watched live over SSE and
the page has to show change as change.

- **Chevrons march** up (or down) the travelling section of a lane, 1.2s linear,
  whenever something is actually moving.
- **A parked lane breathes.** Nothing is travelling while a pipeline waits on a
  person, so its chevrons hold still and pulse in place instead of marching.
  They are not allowed to stop dead: a lane that needs somebody looking exactly
  like a finished one is the original bug this component was written to fix, and
  it came back once — the segment went static while the dot kept pulsing, so the
  "does anything animate" check passed. There is now an assertion on the
  chevrons themselves.
- **The head grows.** When a deploy lands, the solid section animates from its
  old head to its new one over 600ms. This is the one orchestrated moment: it
  shows the promotion happening rather than blinking a new state into place.
- **The head breathes** — a soft halo — only while that lane has work in flight.
- **Fan-out and gather** are width/position transitions, because the strands are
  the same object rearranged.
- Everything above collapses to instant state changes under
  `prefers-reduced-motion`.

Explicitly *not* done: entrance animations on every card, hover lifts on
everything, decorative gradient washes.

### The shape of a lane: four layers

The gutter had been accumulating special cases — a segment here, an overlap
fudge there, a square join to stop a notch showing — and every fix moved the
problem somewhere else. What it needed was a model. A lane is **a road with
things painted on it**, in four layers:

| | | |
|---|---|---|
| 0 | **track** | the road — faint, from the topmost marker on the lane down to the bottom |
| 1 | **approach** | what is on its way in: a deploy travelling up, a stage parked on a person, a failure nothing has replaced |
| 2 | **hold** | what the environment is running, and the history below it |
| 3 | **override** | what is leaving: a rollback travelling back down through ground the lane still holds |

Two rules govern every extent, and between them they replace all of the
patching:

The track obeys the same dot rule as everything else: it starts a cap above the
topmost marker, not at the top of the list. It used to span the whole column, so
a lane with nothing left to do still trailed a pale stub above its head —
background with no road under it. Now, when the head *is* the topmost marker,
the track and the hold coincide and there is nothing to see.

**A run goes from one dot to another and consumes both.** It reaches `cap` past
the marker at each end and never stops on one — a boundary that lands on a dot
cuts it in half, and the dot then reads as sitting crooked on the lane. `cap` is
the strand's radius, which puts the dot at the centre of curvature of the
rounded end: the two are concentric, so the clearance around a dot is the same
above as it is at the sides.

Which colour shows at each end is then a question of *layer*, not of extent. An
approach run is painted under the hold, so where they meet the environment's own
colour wins and its dot sits on solid. A rollback is painted over it, so it wins
at both ends — the stretch it is travelling and the release it is leaving are
one yellow object, with the head dot inside it rather than stranded above it.

**A run never has to butt against another.** Every run is a pill, and the layer
beneath always covers its ends: the track is under everything, and an approach
run continues past where the hold begins. So a rounded end can only ever reveal
the layer below, never the page. No square joins, no overlap constants.

Five kinds of run cover every state a lane can be in, and each maps to one fill
and at most one direction of chevron:

| Run | Layer | Fill | Chevrons |
|---|---|---|---|
| `solid` | hold | the environment's colour | — |
| `travel` | approach | its colour, tinted — it is not here yet | up, marching |
| `wait` | approach | yellow — somebody has to act | up, breathing |
| `reverse` | override | yellow | down, marching |
| `fault` | approach | red | — |

Down is always yellow and never an environment's colour, so a rollback cannot be
mistaken for a deploy at a glance. The dot a rollback is heading for takes the
yellow with it — the destination is part of the rollback, not an ordinary deploy
that happens to sit underneath one.

Dots follow the same discipline: sizes come from the strand width and nothing
paints outside it, so a fanned strand gets smaller dots rather than the same
ones bulging off the sides. `pending` is dashed and `past` is solid, so the
state that has not happened cannot be read as the one that did.

A dot is also **sized to the same parity as its strand**, and positioned with a
whole-pixel offset rather than `left: 50%` and a transform. A 7px dot in a 12px
strand insets by 2.5px, which is symmetric on paper and half a pixel off the bar
once a browser rounds it — and Chrome and Firefox round it differently, so it
was a real visible shift in one browser and nothing at all in the other. The
capture suite asserts whole-pixel insets, on fanned strands as well as collapsed
lanes, and can be run against another engine with `GALLERY_BROWSER`.

### Shape### Shape

One radius per role, so radius carries meaning rather than being a house style:
lanes and strands are full pills, cards are 8px, chips are pills, buttons 6px.
Machine tokens (sha, version, elapsed, destination names) are monospace with
tabular figures so columns of them line up while values change; human text
(commit titles, stage labels) is the app's sans.

---

## What this changes in the code

| File | Change |
|---|---|
| `frontend/src/lib/colors.js` | Stage-aware palette, theme-aware, plus `envRank` for lane order |
| `frontend/src/lib/lane-geometry.js` | **New.** Pure lane-segment maths, extracted from the component so it is testable |
| `frontend/src/lib/lane-states.js` | Per-destination resolution alongside per-environment |
| `frontend/src/ReleaseTimeline.svelte` | Gutter rewrite, card chrome, fan-out, hover cards |
| `frontend/src/lib/fixtures.js` | Destination-layer, rollback and soak fixtures |
| `frontend/gallery/` | The state gallery the rendered-state tests run against |

### What this replaces

The timeline used to be rendered twice. The server built one in MiniJinja —
`build_timeline`, plus per-page fetches of destination states, release intents
and every project's pipelines — and the browser drew the gutter over it with a
`<swim-lanes>` web component, kept live by `live-events.js` and remembered by
`details-persist.js`.

`<release-timeline>` replaced all of it and the old half was left behind: no
template had read `timeline` or `lanes` for some time, and none of the three
scripts was loaded anywhere. Removing it takes the server-side renderer, its
three fetch sites — including an N+1 loop that pulled every project's pipelines
one sequential request at a time on the org Releases page, and every artifact in
the org, to fill a variable nothing read — and the three dead scripts. One
implementation remains: the JSON timeline endpoint and this component.

### Lane order

Production first, closest to the page edge; the card column on the right is the
source. Code then flows **right to left**, from commit to production, which is
the reading Uber's own ordering implies. Ordering is by stage rank, with the
server's environment order as the tiebreak.
