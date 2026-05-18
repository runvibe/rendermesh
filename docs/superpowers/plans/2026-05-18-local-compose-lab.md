# Local Compose Lab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a local RenderMesh lab that runs MinIO as an S3-compatible bucket, seeds a mini frontend app, runs a local edge API, and lets developers exercise template rendering plus static delivery.

**Architecture:** Docker Compose owns local dependencies: Jaeger, MinIO, a one-shot MinIO seeder, and a tiny edge API. RenderMesh still runs from Cargo on the host and reads `examples/local/rendermesh.yaml`, which points at MinIO and the edge API. The seeded bucket contains `index.html` for Handlebars rendering, `static.html` for non-template static delivery, and `/_rendermesh/edge.yaml` for edge behavior.

**Tech Stack:** Docker Compose, MinIO, Node.js edge API, existing Rust RenderMesh binary.

---

### Task 1: Compose Services

**Files:**
- Modify: `docker-compose.yaml`

- [x] Add `minio`, `minio-init`, and `edge-api` services.
- [x] Keep existing Jaeger service intact.
- [x] Add a named `minio_data` volume.
- [x] Validate with `docker compose config`.

### Task 2: Local Example Assets

**Files:**
- Create: `examples/local/rendermesh.yaml`
- Create: `examples/local/bucket/index.html`
- Create: `examples/local/bucket/static.html`
- Create: `examples/local/bucket/docs/index.html`
- Create: `examples/local/bucket/_rendermesh/edge.yaml`
- Create: `examples/local/edge-api/package.json`
- Create: `examples/local/edge-api/server.mjs`

- [x] Define one origin mapped to `test.com`.
- [x] Point storage env names at MinIO-compatible variables.
- [x] Configure the bucket edge hook to call `http://127.0.0.1:4010/edge`.
- [x] Make `/` and `/index.html` render with params.
- [x] Make `/static.html` continue without params so it is served directly.
- [x] Add `/direct` as a direct edge body response.

### Task 3: Documentation And Verification

**Files:**
- Modify: `README.md`
- Create: `examples/local/README.md`

- [x] Document startup commands.
- [x] Document how to reseed after editing `examples/local/bucket/index.html`.
- [x] Document curl commands for template, static, direct edge, auto-index, redirect, and rewrite flows.
- [x] Run `cargo build`.
- [x] Run `cargo test`.
