# Authentication

Forest stores authentication per named server context. Keep development,
staging, and production in separate contexts:

```bash
forest context create team \
  --server https://api.forest.example.com \
  --use
forest context active
```

## Browser device login

The preferred human login is the device flow:

```bash
forest auth login --web
forest auth status
```

The CLI prints an approval URL and, on an interactive terminal, a QR code. Use
`--no-qr` when block characters are unsuitable for the terminal or logs.

## Password login

Use the legacy password flow only where the server requires it:

```bash
forest auth login --password --email engineer@example.com
```

Scripts may provide the password through the supported `FOREST_PASSWORD`
mechanism rather than placing it in command arguments. Prefer tokens for
non-interactive use.

## Registration

If the selected server allows self-registration:

```bash
forest auth register
```

Registration policy, email verification, and allowed domains are controlled by
the server operator.

## Personal access tokens

The current CLI exposes personal access tokens:

```bash
forest auth token create \
  --name ci-component-publisher \
  --expires-in 2592000
forest auth token list
```

The create command writes the raw token once. Store it directly in the CI
provider's secret store and use it as `FOREST_TOKEN`:

```bash
export FOREST_TOKEN="<token>"
export FOREST_SERVER="https://api.forest.example.com"
forest publish --dry-run
forest publish
```

For trusted internal development only, create tokens from a dedicated
least-privileged account, give them an expiry, keep separate tokens per
repository, and revoke them when a workflow is retired:

```bash
forest auth token delete <token-id>
```

Do not share one employee's non-expiring token across an organisation. Do not
onboard external design-partner CI on personal tokens.

## Current identity limitations

Forest does **not** currently expose an organisation machine-identity command.
Older documentation describing `forest organisation app create` is incorrect.
Scoped, expiring workload identities are required before external CI access.

`FOREST_SERVICE_ACCOUNT_API_KEY` is an internal server credential with
cross-organisation behaviour. It is not a customer CI token and must not be
distributed to repositories or developer machines.

Token scope enforcement, rotation, last-used visibility, and cross-tenant
authorization remain part of the
[security readiness work](../product/security-and-sandboxing.md).

## Logout

```bash
forest auth logout
```

Logout removes the current context's local session. Revoke personal tokens
separately; logging out does not invalidate tokens already copied to CI.
