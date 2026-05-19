# Spec 001: Landing Page and Marketing Site

## Status: Phase 2 (Implementation)

## Behavioral Contract

### Routes
- `GET /` returns the landing page with HTTP 200
- `GET /pricing` returns the pricing page with HTTP 200
- `GET /static/*` serves static files from the `static/` directory
- All pages use the shared base template layout
- Unknown routes return HTTP 404

### Landing Page Content
- Hero section with tagline and CTA buttons
- Code example showing a forest.cue manifest
- Feature grid highlighting: registry, deployments, managed services, type safety, teams, pricing
- Final CTA section

### Pricing Page Content
- Displays 4 tiers: Free ($0), Developer ($10/mo), Team ($25/user/mo), Enterprise (Custom)
- Usage-based add-on pricing table
- Accurate pricing data matching specs/PITCH.md

### Non-Functional Requirements
- Pages render server-side (no client-side JS required for content)
- Response time < 10ms for template rendering
- Valid HTML5 output
- Responsive layout (mobile + desktop)

## Edge Cases
- Template file missing -> 500 with error logged
- Static file not found -> 404
- Malformed path -> handled by axum routing (no panic)

## Purity Boundary
- Template rendering is effectful (file I/O) -> lives in forage-server
- No pure core logic needed for static pages
- Template engine initialized once at startup

## Verification
- Integration test: GET / returns 200 with expected content
- Integration test: GET /pricing returns 200 with expected content
- Integration test: GET /nonexistent returns 404
- Compile check: `cargo check` passes
