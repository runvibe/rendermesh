# Release

RenderMesh includes a GitHub Actions workflow for building and publishing a multi-platform Docker image.

Workflow:

```text
.github/workflows/release.yaml
```

Runtime image:

```text
Dockerfile.artifact
```

## Trigger

The workflow runs on:

- Pushes to `main` that touch source, Cargo files, `Dockerfile.artifact`, or the release workflow.
- Manual `workflow_dispatch`.

## Build Strategy

The release workflow builds one binary per architecture with `cross`:

- `linux/amd64` using `x86_64-unknown-linux-gnu`.
- `linux/arm64` using `aarch64-unknown-linux-gnu`.

Each binary is copied into:

```text
artifacts/<bin-name>/<arch>/<bin-name>
```

`Dockerfile.artifact` expects that artifact layout and copies the correct binary using Docker's `TARGETARCH` build arg.

## Image Tags

Architecture-specific tags:

- `latest-amd64`
- `latest-arm64`
- `<git-sha>-amd64`
- `<git-sha>-arm64`

Manifest tags:

- `latest`
- `<git-sha>`

## Registry

Images are published to GitHub Container Registry:

```text
ghcr.io/<owner>/<crate-name>
```

The crate name is resolved from `Cargo.toml`; for this project it is `rendermesh`.

## Permissions

The workflow requires:

- `contents: write`
- `packages: write`

It authenticates to GHCR using `secrets.GITHUB_TOKEN`.
