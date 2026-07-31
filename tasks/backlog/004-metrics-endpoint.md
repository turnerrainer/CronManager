# 004 — Prometheus `/metrics` endpoint (opt-in via config)

## Filed
2026-07-29 — deferred from v0.1.0-alpha.1 scope.

## Severity
Low.

## Motivation
Operators want per-job success rate, execution latency, retry
count, and outbound HTTP status distribution without querying
the DB. Prometheus is the de-facto standard.

## Fix / Design
Wire `metrics` + `metrics-exporter-prometheus`. Metrics:

- `cronmanager_job_executions_total{job, group, status}` (counter)
- `cronmanager_job_duration_seconds{job, group}` (histogram)
- `cronmanager_job_retry_attempts{job, group}` (counter)
- `cronmanager_scheduler_registered_jobs` (gauge)

Enable via `metrics.enabled: true` in config; endpoint is
`GET /metrics`. Keep the endpoint on the same port (per
DEV-REQUIREMENTS §5.3 "no admin HTTP endpoints in the same
process" — Prometheus scraping is a consumer surface, not
admin, so this is compliant).

## Acceptance
- [ ] Configurable via `metrics: { enabled: bool }` block.
- [ ] `GET /metrics` returns text/plain in the Prometheus
      exposition format.
- [ ] Every counter increments on the right event (verified via
      integration test).

## Estimated effort
0.5 day.

## Dependencies
None.

## Non-scope
- Pushgateway support.
- OpenTelemetry exporters.
