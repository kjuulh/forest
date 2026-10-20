# Authentication

Forest supports multiple authentication methods depending on your use case.

## Interactive Login

For human users, authenticate via the CLI:

```bash
forest auth login
```

This opens a browser-based login flow and stores your credentials locally.

## Check Status

```bash
forest auth status
```

Shows who you are logged in as on the active context. It does not print the server: `forest context active` does. A `FOREST_TOKEN` in the environment is used instead of the stored login, and `auth status` does not see it.

## Logout

```bash
forest auth logout
```

## Register

If your Forest server supports self-registration:

```bash
forest auth register
```

## Personal Access Tokens

For scripting and CI/CD, create a personal access token:

```bash
forest auth token create
```

Use the token via the `Authorization: Bearer <token>` header or by setting it in your environment.

## Authentication Methods

Forest supports three types of identities:

| Identity | Use Case | How It Works |
|----------|----------|-------------|
| **User** | Human operators | JWT from `forest auth login` |
| **OAuth app** | Services and CI calling as themselves | Client-credentials token, scoped to the app's organisation and its granted scopes |
| **Service Account** | Infrastructure services | Single API key, cross-org access for internal tooling |

### OAuth apps

An organisation's OAuth app with the `client_credentials` grant calls Forest as itself, not as a person:

```bash
forest organisation oauth-app create --org my-org --name "ci-publisher" \
  --redirect-uri https://example.invalid/unused \
  --grant-type client_credentials --scope components:publish
```

Its tokens are short-lived, limited to the app's organisation, and limited to the scopes granted. `components:publish` covers publishing and reading the registry, not releasing. A CI job passes the token as `FOREST_TOKEN`. For anything else CI does, a personal access token (`forest auth token create`) carries its creator's permissions.

### Service Accounts

Service accounts use a server-configured API key (`FOREST_SERVICE_ACCOUNT_API_KEY` environment variable) and bypass organisation checks. These are for internal infrastructure services that need cross-org access.
