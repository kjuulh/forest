# Flux v2 Destination Setup

The `forest/flux@1` destination type deploys to Kubernetes clusters via [Flux v2](https://fluxcd.io/) GitOps. Forest commits rendered manifests to a Git repository that Flux watches, then optionally triggers immediate reconciliation via a webhook.

## How It Works

1. `forest release create` renders deployment manifests from component templates
2. The manifests are committed and pushed to a GitOps repository
3. Flux detects the change and applies the manifests to the cluster
4. Optionally, a webhook triggers Flux to reconcile immediately (instead of waiting for the poll interval)

## Prerequisites

- A Kubernetes cluster with [Flux v2](https://fluxcd.io/) installed
- A Git repository for GitOps manifests
- The `notification-controller` component (included in default Flux installation)

## Step 1: Create the Destination

```bash
forest destination create \
  --organisation my-org \
  --name flux-dev/home/001 \
  --environment dev \
  --type "forest/flux@1"
```

## Step 2: Configure Destination Metadata

The Flux destination requires metadata for Git access and cluster identification:

```bash
forest destination update --name "flux-dev/home/001" \
  --metadata "cluster_name=my-cluster" \
  --metadata "namespace=dev" \
  --metadata "git_url=https://git.example.com/org/flux-manifests.git" \
  --metadata "git_username=bot" \
  --metadata "git_token=<git-access-token>" \
  --metadata "git_author_name=forest" \
  --metadata "git_author_email=forest@example.com" \
  --metadata "environment=dev"
```

### Required Metadata

| Key | Description |
|-----|-------------|
| `cluster_name` | Logical name for the target cluster |
| `namespace` | Kubernetes namespace for deployed resources |
| `git_url` | HTTPS or SSH URL of the GitOps repository |
| `environment` | Environment name (matches `env` in `forest.cue`) |

### Git Authentication (pick one)

| Key | Description |
|-----|-------------|
| `git_username` + `git_token` | HTTPS authentication |
| `git_ssh_key_path` | Path to SSH private key |

### Optional Metadata

| Key | Description | Default |
|-----|-------------|---------|
| `git_branch` | Branch to commit to | `main` |
| `git_author_name` | Git commit author name | `forest-release` |
| `git_author_email` | Git commit author email | `forest@release.local` |
| `reconcile_url` | Flux Receiver webhook URL for immediate reconciliation | _(none — flux polls)_ |
| `webhook_secret` | HMAC secret for Flux Alert notifications back to forest | _(none)_ |
| `forest_webhook_url` | Forest webhook URL for Flux notifications | _(none)_ |
| `flux_git_repository_name` | Name of the Flux GitRepository CR | `flux-system` |

## Step 3: Set Up Immediate Reconciliation (Recommended)

By default, Flux polls the Git repository on an interval (typically 5 minutes). For faster deployments, configure a Flux Receiver webhook so forest can trigger reconciliation immediately after pushing manifests.

### Deploy the Flux Receiver

Add a Receiver to your cluster infrastructure. If you use the forest infrastructure component:

```cue
// forest.cue
config: {
    flux_receiver: {
        enabled: true
        token: "your-webhook-secret-token"
    }
}
```

Or deploy manually:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: forest-receiver-token
  namespace: flux-system
stringData:
  token: your-webhook-secret-token
---
apiVersion: notification.toolkit.fluxcd.io/v1
kind: Receiver
metadata:
  name: forest
  namespace: flux-system
spec:
  type: generic
  secretRef:
    name: forest-receiver-token
  resources:
    - kind: GitRepository
      name: flux-system
      namespace: flux-system
```

### Get the Receiver Webhook URL

After the Receiver is created, get its webhook path:

```bash
kubectl -n flux-system get receiver forest \
  -o jsonpath='{.status.webhookPath}'
```

This returns a path like `/hook/8e8cf5f4afb33f39...`. The full URL is:

```
http://webhook-receiver.flux-system<webhook-path>
```

### Configure the Destination

Add the `reconcile_url` to your destination metadata:

```bash
forest destination update --name "flux-dev/home/001" \
  --metadata "reconcile_url=http://webhook-receiver.flux-system/hook/8e8cf5f4..."
```

Now `forest release create` will trigger immediate reconciliation after pushing manifests. Deployments apply within seconds instead of waiting for the poll interval.

## Step 4: Use in Projects

Reference the destination in your `forest.cue`:

```cue
kjuulh: service: {
    env: {
        dev: {
            destinations: [
                {destination: "flux-dev.*", type: "forest/flux@1"},
            ]
            config: {}
        }
    }

    config: {
        name:  "my-service"
        image: "registry.example.com/my-service"
        tag:   "latest"
        // ...
    }
}
```

## Releasing

```bash
# Release with default tag from forest.cue
forest release create --env dev

# Override the image tag from CI
forest release create --env dev --set my-org/service.tag=abc123
```

The Flux destination handler will:

1. Render manifests from component templates using your config
2. Invoke component deployment hooks (sealed secrets, forage resources, etc.)
3. Commit all manifests to the GitOps repository
4. Trigger Flux reconciliation via the webhook (if `reconcile_url` is configured)
5. Stream release status back to the CLI

## Repository Layout

Forest organizes the GitOps repository automatically:

```
releases/
  dev/
    flux-dev/home/001/
      clank-forage-dev/
        dev/
          rawpotion-forest/
            10-namespace.yaml
            20-sealed-secrets.yaml
            25-forage-postgresql.yaml
            25-forage-nats.yaml
            25-forage-s3.yaml
            30-deployment.yaml
            40-ingress.yaml
            kustomization.yaml
            .forest/
              release.yaml
clusters/
  dev/
    flux-dev/home/001/
      clank-forage-dev/
        dev/
          rawpotion-forest.yaml    # Flux Kustomization CR
          kustomization.yaml       # Plain kustomize resources list
```

## Troubleshooting

### Release succeeds but manifests don't apply

Check if `reconcile_url` is configured:

```bash
forest destination list --organisation my-org
```

If not configured, flux waits for its poll interval. Either configure the webhook or manually trigger:

```bash
flux reconcile source git flux-system
flux reconcile kustomization <project-name>
```

### Flux Kustomization stuck in "Reconciliation in progress"

This can happen if `wait: true` is set on the Kustomization CR and the deployment is unhealthy. Forest now generates Kustomizations with `wait: false` to prevent this deadlock. If you have an old Kustomization, patch it:

```bash
kubectl -n flux-system patch kustomization <name> \
  --type=merge -p '{"spec":{"wait":false}}'
```

### Webhook returns 404

Verify the Receiver is ready and the path matches:

```bash
kubectl -n flux-system get receiver forest
```

The `STATUS` column should show the webhook path. Ensure the `reconcile_url` in the destination metadata matches exactly.
