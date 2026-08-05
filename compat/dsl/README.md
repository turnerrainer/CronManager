# CronManager Sample Job Definitions

This directory contains sample YAML job definitions that demonstrate the capabilities of CronManager.

## Directory Structure

```
DSL/samples/
├── http/              # HTTP request jobs
├── shell/             # Shell script execution jobs
└── schedules/         # Schedule pattern examples
```

## Sample Files

### HTTP Jobs (`http/`)
- **health-check.yaml**: Simple GET health check every 5 minutes with retry
- **api-status.yaml**: REST API status check every 10 minutes
- **external-service.yaml**: External service availability check every 15 minutes
- **manual-trigger.yaml**: Manual trigger example (no automatic schedule)
- **post-webhook.yaml**: POST request hourly
- **put-update.yaml**: PUT request every 6 hours with aggressive retry
- **delete-cleanup.yaml**: DELETE request daily at 2 AM (fail hard on error)
- **time-bounded.yaml**: Job that only runs between specific dates

### Shell Jobs (`shell/`)
- **hello-world.yaml**: Simple greeting script every 10 minutes
- **system-info.yaml**: System information gathering every 6 hours
- **list-files.yaml**: Directory listing daily at 8 AM
- **manual-script.yaml**: Manual execution only
- **backup.yaml**: Backup job with environment variables
- **process-data.yaml**: Data processing with environment variables
- **cleanup-temp.yaml**: Cleanup job every 12 hours
- **seasonal-maintenance.yaml**: Time-bounded maintenance task
- **generate-report.yaml**: Weekly report generation on Mondays

### Schedule Examples (`schedules/examples.yaml`)
Demonstrates various cron expression patterns (multiple jobs in one file):
- Every minute, every 30 seconds
- Hourly, daily schedules
- Weekday-specific schedules
- Monthly patterns
- Multiple times per day

## Job Configuration Structure

### HTTP Jobs
```yaml
job_name:
  trigger: "cron_expression"  # or "off" for manual only
  type: http
  method: GET|POST|PUT|DELETE|PATCH
  url: https://example.com/endpoint
  retryCount: 3               # Optional: number of retry attempts
  retryDelay: 2000            # Optional: delay between retries (ms)
  ignoreFailures: true        # Optional: don't throw exception on failure
  startDate: 1704067200000    # Optional: Unix timestamp (ms)
  endDate: 1735689600000      # Optional: Unix timestamp (ms)
```

### Shell Execution Jobs
```yaml
job_name:
  trigger: "cron_expression"  # or "off" for manual only
  type: exec
  command: ./scripts/samples/script.sh
  allowedEnvs:                # Optional: whitelist of environment variables
    - ENV_VAR_1
    - ENV_VAR_2
  startDate: 1704067200000    # Optional: Unix timestamp (ms)
  endDate: 1735689600000      # Optional: Unix timestamp (ms)
```

## Usage

### Creating Your Own Jobs

1. Create a YAML file in the appropriate subdirectory (`http/` or `shell/`)
2. Define your job using the structure above
3. Restart CronManager: `docker-compose restart`

For detailed guides, see:
- **[Creating HTTP Jobs](../../docs/how-to/create-http-job.md)** - Complete HTTP job guide
- **[Creating Shell Jobs](../../docs/how-to/create-shell-job.md)** - Complete shell job guide
- **[Advanced Scheduling](../../examples/advanced-scheduling/)** - Cron expression patterns

### Testing Jobs

See **[Getting Started Guide](../../docs/GETTING_STARTED.md#verify-installation)** for verification commands.

## Quick Reference

**Cron format**: `second minute hour day-of-month month day-of-week`

Common patterns:
- `0 */5 * * * ?` - Every 5 minutes
- `0 0 9 * * ?` - Daily at 9 AM
- `0 0 9 ? * MON-FRI` - Weekdays at 9 AM

For comprehensive cron patterns, see **[Advanced Scheduling Guide](../../examples/advanced-scheduling/README.md)**.

## Notes

- Each YAML file typically contains one job (except `schedules/examples.yaml`)
- Group names are derived from directory path + filename (e.g., `http/health-check.yaml` → group: `http-health-check`)
- Jobs are referenced as `{groupName}/{jobName}` in the API
- Scripts must be executable (`chmod +x`)
- Multiple jobs can be defined in a single YAML file

## See Also

- **[API Reference](../../docs/reference/openapi.yaml)** - Complete API documentation
- **[Troubleshooting](../../docs/TROUBLESHOOTING.md)** - Common issues and solutions
