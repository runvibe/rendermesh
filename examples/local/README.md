# RenderMesh Local Lab

This directory contains a complete local lab for exercising RenderMesh with:

- MinIO as an S3-compatible bucket.
- A seeded frontend bucket.
- A tiny local edge API.
- A RenderMesh manifest with one origin and wildcard host mapping.

## Start Dependencies

From the repository root:

```bash
docker compose up -d postgres jaeger minio edge-api
docker compose run --rm minio-init
```

MinIO console:

- URL: `http://localhost:9001`
- user: `rendermesh`
- password: `rendermesh-secret`

## Run RenderMesh

Run RenderMesh on the host so it can call the edge API at `127.0.0.1:4010`:

```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5453/postgres
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

Use `Host: local.rendermesh.test` for RenderMesh requests:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/
```

## Flows To Try

Template render through edge params:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/home
```

Static file without template rendering:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/static.html
```

The response should still contain the literal `{{title}}` token from `static.html`.

Direct edge body:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/direct
```

Edge-selected file path:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/file
```

Auto index rewrite:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/docs
```

Redirect:

```bash
curl -i -H 'Host: local.rendermesh.test' http://127.0.0.1:3000/legacy
```

## Update The Bucket

Edit:

```text
examples/local/bucket/index.html
```

Then reseed MinIO:

```bash
docker compose run --rm minio-init
```

RenderMesh syncs the local mirror every 5 seconds in this lab. Wait a few seconds and request `/` again.
