#!/usr/bin/env bash
# Sample backup job — no-op, just prints the config it received.
set -euo pipefail
BACKUP_DIR=${BACKUP_DIR:-/tmp/backups}
RETENTION_DAYS=${RETENTION_DAYS:-7}
echo "would back up to ${BACKUP_DIR}, retaining ${RETENTION_DAYS} day(s)"
mkdir -p "${BACKUP_DIR}"
touch "${BACKUP_DIR}/last-run.txt"
date -u >> "${BACKUP_DIR}/last-run.txt"
