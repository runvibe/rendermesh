# Global Host Wildcard Design

## Summary

RenderMesh currently routes requests through exact hosts and domain wildcards
such as `*.example.com`. Requests whose normalized `Host` matches neither form
return `421 Misdirected Request`.

The global manifest should also accept the literal host key `*`. This key maps
every otherwise-unmatched valid host to one origin, allowing a deployment to
serve a single bucket or local origin independently of the incoming domain.

```yaml
origins:
  app:
    type: s3
    bucket: app-assets
    endpoint_env: APP_STORAGE_ENDPOINT
    region_env: APP_STORAGE_REGION

hosts:
  "*":
    origin: app
```

The global wildcard is a routing fallback, not a DNS pattern, CORS policy, or
substitute for a valid HTTP `Host` header.

## Goals

- Accept the literal `*` key in the manifest `hosts` map.
- Route every valid host not matched by a more specific rule to the configured
  origin.
- Preserve exact-host and domain-wildcard behavior.
- Keep the requested host available as `ResolvedHost.normalized_host`.
- Identify fallback matches as `*` in `ResolvedHost.matched_host`.
- Keep unknown-host `421` behavior when no global wildcard is configured.
- Document the syntax, precedence, and integration boundaries.

## Non-Goals

- Matching requests with a missing or syntactically invalid `Host` header.
- Treating `*` as a DNS record or CDN custom hostname.
- Allowing more than one global fallback.
- Adding path-based origin selection.
- Changing origin synchronization, rendering, redirects, rewrites, or edge
  hook behavior.
- Automatically allowing cross-origin requests from every domain.
- Introducing a separate `default_origin` manifest field.

## Considered Approaches

### Dedicated fallback in `HostResolver` (selected)

Recognize `*` while compiling the existing `hosts` map and store its origin in
a dedicated optional fallback field. Resolution remains explicit: exact,
domain wildcard, then global fallback.

This approach preserves the requested manifest syntax, makes precedence clear,
and avoids pretending that `*` is a normal hostname or suffix.

### Top-level `default_origin`

Add a separate manifest field rather than interpreting `*` inside `hosts`.
This is structurally explicit, but introduces a second host-routing contract
and does not provide the requested `hosts: { "*": ... }` syntax.

### Empty-suffix wildcard

Compile `*` as a wildcard with an empty suffix. This minimizes fields in the
resolver, but obscures its distinct semantics and risks coupling global
fallback behavior to domain-wildcard sorting and matching rules.

## Manifest Contract

The existing `hosts` map remains the only routing configuration surface:

```yaml
hosts:
  admin.example.com:
    origin: admin
  "*.example.com":
    origin: tenant
  "*":
    origin: default
```

The `*` value uses the existing `HostConfig`, including the existing validation
that its `origin` must reference a configured origin. YAML and JSON manifests
behave identically.

Whitespace around a host key is normalized consistently with existing entries,
so a key such as `" * "` is treated as `*`. Case normalization has no effect on
the literal marker. Resolver construction must reject multiple keys that
normalize to the global wildcard, rather than selecting one based on map order.

Other malformed wildcard forms remain invalid. Examples such as `foo.*`,
`**`, and `*.`, as well as invalid exact hosts, must still fail while building
the host resolver.

## Resolution Semantics

`HostResolver` should compile three independent routing structures:

1. Exact host mappings.
2. Domain-wildcard mappings, ordered from the most specific suffix to the least
   specific suffix.
3. An optional global fallback origin.

For each request, resolution follows this order:

1. Normalize and validate the incoming `Host`, including the existing handling
   of a numeric port.
2. Return an exact match when present.
3. Return the most specific matching domain wildcard.
4. Return the global `*` fallback when configured.
5. Otherwise return no match, preserving the gateway's `421 Misdirected
   Request`.

Examples for the manifest above:

| Incoming host | Matched rule | Origin |
| --- | --- | --- |
| `admin.example.com` | `admin.example.com` | `admin` |
| `shop.example.com` | `*.example.com` | `tenant` |
| `unrelated.test` | `*` | `default` |
| `unrelated.test:8443` | `*` | `default` |

For a global fallback result:

- `normalized_host` contains the normalized incoming host without its port.
- `matched_host` contains the literal `*`.
- `origin_id` contains the fallback origin id.

This distinction preserves the actual request host for edge-hook context and
observability while exposing which manifest rule selected the origin.

## Invalid and Missing Hosts

The global wildcard applies only after the incoming `Host` passes the existing
normalization and validation. It must not turn malformed authority values into
valid routes.

A missing, empty, malformed, or unsupported host therefore remains unresolved
and follows the existing gateway error path. This includes invalid ports and
host labels rejected by `normalize_host`.

## CORS

The literal `*` must not create an allow-any-origin CORS rule. CORS remains
derived only from explicit exact hosts and domain wildcards.

An origin reached through the global fallback may still allow CORS when the
request's `Origin` header matches another explicit host rule mapped to that
same origin. Otherwise, RenderMesh does not emit the corresponding CORS allow
origin response.

This keeps routing convenience separate from browser trust policy and avoids
silently broadening cross-origin access.

## CDN Domain Reconciliation

The literal `*` is not a provider domain and must be excluded from desired CDN
domains regardless of `include_wildcards`.

Exact hosts continue to participate normally. Domain wildcards such as
`*.example.com` continue to participate only when `include_wildcards` is true.
Operators using only `*` remain responsible for configuring DNS or CDN ingress
outside RenderMesh.

## Components and Responsibilities

### Manifest DTO

No DTO schema change is required. `RenderMeshManifest.hosts` and `HostConfig`
already represent the requested configuration.

### Manifest Service

`HostResolver` owns interpretation and precedence. It should gain a dedicated
optional fallback origin and recognize the normalized literal `*` during
construction. Existing exact and domain-wildcard structures remain unchanged.

Manifest validation continues to ensure the referenced origin exists.
Resolver construction continues to reject invalid host patterns and rejects
duplicate normalized global fallback entries.

### CORS Service

`CorsPolicy::from_manifest` should explicitly skip `*`. The current invalid-host
filter may already produce this result, but an explicit branch and regression
test make the security boundary intentional.

### CDN Domain Service

Domain normalization should explicitly reject `*`, with tests covering both
values of `include_wildcards`.

### Render Gateway

No route or transport changes are required. The gateway continues to consume
`HostResolver::resolve` and receives a normal `ResolvedHost` for fallback
requests.

## Data Flow

1. Startup loads and validates the global manifest.
2. `HostResolver` compiles exact, domain-wildcard, and optional global fallback
   mappings.
3. A request reaches the render gateway with a `Host` header.
4. The resolver normalizes the host and applies the precedence rules.
5. The selected origin follows the existing rendering pipeline.
6. Edge hooks receive the actual normalized request host and `matched_host: "*"`
   when the fallback selected the origin.
7. If no rule resolves the host, the gateway preserves the existing `421`
   response.

## Error Handling

- `*` referencing an unknown origin fails manifest validation with the existing
  unknown-origin error.
- Multiple host keys that normalize to `*` fail resolver construction with an
  explicit duplicate-global-wildcard error.
- Invalid wildcard and exact-host entries continue to fail resolver
  construction.
- Invalid incoming hosts do not fall back to `*`.
- No runtime fallback is introduced for origin loading, synchronization, or
  rendering failures; those errors keep their existing behavior.

## Testing

Unit tests for `HostResolver` should verify:

- `*` resolves an otherwise unknown valid host.
- The incoming host is normalized and `matched_host` equals `*`.
- An exact host takes priority over `*`.
- A domain wildcard takes priority over `*`.
- The most specific domain wildcard behavior remains unchanged when `*` exists.
- A missing or invalid host does not resolve through `*`.
- A manifest without `*` still leaves unknown hosts unresolved.
- Multiple keys that normalize to `*` are rejected.

Gateway tests should verify:

- An unknown valid host mapped through `*` reaches the configured origin.
- Unknown hosts still return `421` when `*` is absent.
- Invalid host input retains the existing error response when `*` is present.

Integration tests should verify:

- CORS does not treat `*` as an allow-any-origin rule.
- CDN domain reconciliation excludes `*` whether `include_wildcards` is false
  or true.

## Documentation

Update `README.md` and `docs/configuration.md` during implementation to include
the literal `*` example, precedence order, valid-host limitation, CORS behavior,
and CDN exclusion.

The documented precedence must read:

```text
exact host -> most specific domain wildcard -> global * fallback
```

## Acceptance Criteria

- A manifest containing `hosts: { "*": { origin: app } }` starts successfully
  when `app` exists.
- Every syntactically valid unmatched host resolves to `app`.
- Exact and domain-wildcard mappings override `*`.
- `ResolvedHost` preserves the actual normalized host and reports `*` as the
  matched rule.
- Missing or invalid hosts do not resolve through `*`.
- `*` neither enables unrestricted CORS nor appears in CDN desired domains.
- Existing manifests and their routing behavior remain backward compatible.
