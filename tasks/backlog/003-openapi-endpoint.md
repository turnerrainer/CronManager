# 003 — Auto-generate OpenAPI 3.1 spec at `GET /api`

## Filed
2026-07-29 — deferred from v0.1.0-alpha.1 scope. Documented in
`docs/DESIGN.md` §5 as a non-goal for v0.1.x.

## Severity
Low.

## Motivation
Third-party clients (curl, dashboards, generated SDKs) benefit
from a machine-readable API spec. XTR-on-Rust ships this and it
proved useful; CronManager's surface is smaller but the same
argument applies.

## Fix / Design
Hand-code the spec (mirror the `router.rs` route table) rather
than pull in a proc-macro framework. The surface is 8 endpoints;
a static `Value` in `src/openapi.rs` is simpler than a
`utoipa`-style attribute tree.

Serve at `GET /api` returning JSON. Optionally add Redoc/Swagger
at `GET /docs` — but that expands the runtime attack surface;
defer to a separate task if requested.

## Acceptance
- [ ] `GET /api` returns valid OpenAPI 3.1 covering every route
      in `router.rs`.
- [ ] `docs/DESIGN.md` §5 gets its "OpenAPI" bullet moved from
      non-goals to landed.
- [ ] A generated client (via `openapi-generator-cli` or
      `oapi-codegen`) can call every endpoint successfully.

## Estimated effort
0.5 day.

## Dependencies
None.

## Non-scope
- Interactive docs UI (Redoc / Swagger).
- Machine-generated Rust SDK from the spec.
