# forest-contrib/gitea-create-repo

Create a repository on a Gitea instance via its REST API.

## Inputs

- `base_url` — e.g. `https://gitea.example.com`.
- `org` *(optional)* — when set, posts to `/api/v1/orgs/<org>/repos`.
  Otherwise posts to `/api/v1/user/repos` (creates under the token's
  owning user).
- `name`, `description`, `private`, `default_branch` — passed through
  to the Gitea API as-is.
- `token_path` — **filesystem path** to a file containing the API
  token. The component reads the file and uses the contents as the
  bearer.

## Why `token_path` (and not `token` directly)

Secrets passed as command-line args or environment variables leak into
process lists, shell history, and runner logs. By reading from a file,
the token only lives in memory long enough to make the HTTP call.

The expected delivery channel is Forest's secret mount:

```yaml
secrets:
  - { name: gitea-token, target_path: /run/secrets/gitea-token, ... }
steps:
  - uses: forest-contrib/gitea-create-repo@0.1.0
    with:
      base_url:   https://gitea.example.com
      org:        my-org
      name:       my-new-repo
      token_path: /run/secrets/gitea-token
```

## Output

`html_url`, `ssh_url`, `clone_url` — ready to feed into
`forest-contrib/git-commit-push` as `remote_url`.
