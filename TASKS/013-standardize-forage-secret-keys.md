# 013: Standardize secret key names across forage controllers

## Problem

The three forage controllers use different naming conventions for their credential secrets:

**PostgreSQL** (`forest-db-credentials`):
- `DATABASE_URL` — connection string (what most apps use directly)
- `username`, `password`, `host`, `port`, `database` — individual fields

**NATS** (`forest-nats-nats-creds`):
- `creds` — full NATS credentials file (JWT + NKey seed)
- `jwt`, `seed`, `public_key` — individual fields

**S3** (`forest-s3-s3-credentials`):
- `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ENDPOINT_URL`, `AWS_REGION` — AWS SDK convention
- `access-key-id`, `secret-access-key`, `bucket-name`, `endpoint`, `region` — kebab-case

The service component deployment template needs custom `valueFrom` mappings because each controller uses different conventions and apps expect different env var names (`S3_ACCESS_KEY` vs `AWS_ACCESS_KEY_ID` vs `access-key-id`).

## Desired state

Each forage controller should produce secrets with:

1. **A primary connection value** — a single env var that contains everything needed to connect:
   - PostgreSQL: `DATABASE_URL` (already done)
   - NATS: `NATS_CREDS` (the creds file) or `NATS_URL` (with embedded auth)
   - S3: `S3_URL` or a set of standard vars

2. **Standard env var names** — matching what common libraries expect:
   - PostgreSQL: `DATABASE_URL` ✓
   - NATS: `NATS_URL`, `NATS_CREDS`
   - S3: `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_BUCKET`, `S3_REGION`

3. **Consistent casing** — all UPPER_SNAKE_CASE for env vars, matching what `envFrom` would inject.

## Changes needed

### S3 controller (`forage-s3-controller`)
Update `src/controller.rs` to produce keys:
- `S3_ENDPOINT` (instead of `endpoint` and `AWS_ENDPOINT_URL`)
- `S3_ACCESS_KEY` (instead of `access-key-id` and `AWS_ACCESS_KEY_ID`)
- `S3_SECRET_KEY` (instead of `secret-access-key` and `AWS_SECRET_ACCESS_KEY`)
- `S3_BUCKET` (instead of `bucket-name`)
- `S3_REGION` (instead of `region` and `AWS_REGION`)
- Keep `AWS_*` keys for compatibility, but primary keys should be `S3_*`

### NATS controller (`forage-nats-controller`)
Update `src/secrets.rs` to produce keys:
- `NATS_CREDS` (the full creds file, instead of `creds`)
- Keep `jwt`, `seed`, `public_key` as secondary fields

### Service component template
Once the controllers use standard env var names, the service deployment template can use simple `envFrom` for all three instead of custom `valueFrom` mappings:

```yaml
envFrom:
  - secretRef:
      name: {{ config.name }}-secrets
  - secretRef:
      name: {{ config.name }}-db-credentials
  - secretRef:
      name: {{ config.name }}-nats-nats-creds
  - secretRef:
      name: {{ config.name }}-s3-s3-credentials
```

## Impact

This is a breaking change for existing users of the forage secrets. It should be done in a single coordinated release across all three controllers and the service component.

## Files to change

- `forage-s3-controller/crates/forage-s3-controller/src/controller.rs` — update Secret data keys
- `forage-nats-controller/src/secrets.rs` — update Secret data keys
- `forest-deployment/components/kjuulh/service/templates/deployment/forest/flux@1/30-deployment.yaml.jinja2` — simplify to envFrom
