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

Shows your current authentication state — who you're logged in as and which server you're connected to.

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
| **App Token** | Third-party integrations | Organisation-scoped API token, stored as SHA-256 hash |
| **Service Account** | Infrastructure services | Single API key, cross-org access for internal tooling |

### App Tokens

App tokens are scoped to an organisation and are ideal for CI/CD pipelines:

```bash
# Create an app and generate a token
forest organisation app create --name "ci-bot"
```

### Service Accounts

Service accounts use a server-configured API key (`FOREST_SERVICE_ACCOUNT_API_KEY` environment variable) and bypass organisation checks. These are for internal infrastructure services that need cross-org access.
