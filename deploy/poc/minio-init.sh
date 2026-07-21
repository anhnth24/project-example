#!/bin/sh
# Create buckets + narrow application credentials (not MinIO root).
set -eu

MC_ALIAS="${MC_ALIAS:-local}"
ROOT_USER="${MARKHAND_MINIO_ROOT_USER:?MARKHAND_MINIO_ROOT_USER required}"
ROOT_PASSWORD="${MARKHAND_MINIO_ROOT_PASSWORD:?MARKHAND_MINIO_ROOT_PASSWORD required}"
APP_ACCESS_KEY="${MARKHAND_MINIO_ACCESS_KEY:?MARKHAND_MINIO_ACCESS_KEY required}"
APP_SECRET_KEY="${MARKHAND_MINIO_SECRET_KEY:?MARKHAND_MINIO_SECRET_KEY required}"
BUCKET="${MARKHAND_MINIO_BUCKET:-markhand-documents}"

echo "waiting for MinIO..."
i=0
until mc alias set "$MC_ALIAS" http://minio:9000 "$ROOT_USER" "$ROOT_PASSWORD" >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -ge 60 ]; then
    echo "MinIO not ready" >&2
    exit 1
  fi
  sleep 1
done

mc mb --ignore-existing "${MC_ALIAS}/${BUCKET}"
mc mb --ignore-existing "${MC_ALIAS}/markhand-artifacts"

POLICY_NAME=markhand-app
mc admin policy create "$MC_ALIAS" "$POLICY_NAME" /policies/minio-app-policy.json \
  || mc admin policy add "$MC_ALIAS" "$POLICY_NAME" /policies/minio-app-policy.json \
  || true

if ! mc admin user info "$MC_ALIAS" "$APP_ACCESS_KEY" >/dev/null 2>&1; then
  mc admin user add "$MC_ALIAS" "$APP_ACCESS_KEY" "$APP_SECRET_KEY"
fi

mc admin policy attach "$MC_ALIAS" "$POLICY_NAME" --user "$APP_ACCESS_KEY" \
  || mc admin policy set "$MC_ALIAS" "$POLICY_NAME" "user=${APP_ACCESS_KEY}" \
  || true

echo "minio-init complete: bucket=${BUCKET} app_user=${APP_ACCESS_KEY}"
