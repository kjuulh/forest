# 014: Handle empty dict check in Jinja2 templates

## Problem

When CUE produces an optional config block as `forage_postgresql: {}`, the Jinja2 check:

```jinja2
{% if config.forage_postgresql is defined and config.forage_postgresql %}
```

evaluates to `false` because empty dicts are falsy in Jinja2. You need:

```jinja2
{% if config.forage_postgresql is defined %}
```

This is a subtle Jinja2 behavior that causes silent failures — the template silently skips the block without any error, and the developer has to debug why the expected resources aren't being created.

## Options

### Option A: Document the pitfall

Add a "Component Authoring Guide" section that warns about this:

> **Jinja2 pitfall:** `{% if config.foo %}` is false for `{}`. Use `{% if config.foo is defined %}` for optional config blocks.

### Option B: Custom Jinja2 test

Add a custom Jinja2 test `is_configured` that returns true for both `{}` and populated dicts:

```jinja2
{% if config.forage_postgresql is configured %}
```

Implementation in `crates/forest/src/services/templates.rs` (or wherever Jinja2 is invoked):

```rust
env.add_test("configured", |value| {
    !value.is_undefined() && !value.is_none()
});
```

### Option C: Normalize empty dicts to a marker value

Before passing config to Jinja2, replace `{}` with `{"_enabled": true}` so that truthiness checks work:

```jinja2
{% if config.forage_postgresql %}  {# now true because dict is non-empty #}
```

## Recommendation

Option A (document) + Option B (custom test) for the best DX.

## Files to change

- `crates/forest/src/services/templates.rs` — add custom Jinja2 test
- Documentation / component authoring guide
