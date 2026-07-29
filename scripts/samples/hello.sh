#!/usr/bin/env bash
# Sample shell job — proves the environment allow-list is
# threaded through correctly.
set -euo pipefail
echo "hello from $(hostname)"
echo "backup dir: ${BACKUP_DIR:-<unset>}"
echo "retention days: ${RETENTION_DAYS:-<unset>}"
