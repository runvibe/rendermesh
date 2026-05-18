# Testing

RenderMesh has unit tests, integration tests, and a manual local lab.

## Rust Tests

Run everything:

```bash
cargo test
```

Run integration tests only:

```bash
cargo test --test integration
```

Run formatting and build checks:

```bash
cargo fmt --check
cargo build
```

## Local Lab

The local lab runs:

- MinIO as an S3-compatible bucket.
- A seeded frontend bucket.
- A small Node.js edge API.
- Jaeger for local observability.

Start dependencies:

```bash
docker compose up -d jaeger minio edge-api
docker compose run --rm minio-init
```

Run RenderMesh:

```bash
export RENDERMESH_MANIFEST=./examples/local/rendermesh.yaml
export LOCAL_APP_STORAGE_ENDPOINT=http://127.0.0.1:9000
export LOCAL_APP_STORAGE_REGION=us-east-1
export LOCAL_APP_ACCESS_KEY_ID=rendermesh
export LOCAL_APP_SECRET_ACCESS_KEY=rendermesh-secret
export LOCAL_APP_FORCE_PATH_STYLE=true
export APP_HOST=127.0.0.1
export APP_PORT=3000
export OTEL_ENABLED=false

cargo run
```

Test the main route:

```bash
curl -i -H 'Host: test.com' http://127.0.0.1:3000/
```

See [examples/local/README.md](../examples/local/README.md) for every manual route and expected result.

## Manual Flow Coverage

The local lab covers:

- Host-to-origin resolution.
- Bucket sync from MinIO.
- Origin edge config loading.
- Edge hook request and response contract.
- HTML template rendering with params.
- Static pass-through without rendering.
- Direct edge body responses.
- Edge-selected `file_path`.
- Auto-index from `/docs` to `/docs/index.html`.
- Redirects from `/legacy` to `/static.html`.
- Missing-file behavior using `/index.html` with `404`.

## Browser Testing

Add:

```text
127.0.0.1 test.com
```

to `/etc/hosts`, then open:

```text
http://test.com:3000/
```
