#!/bin/sh
# Create buckets + narrow application credentials (not MinIO root).
# Fail-closed: policy create/attach errors abort init.
set -eu

MC_ALIAS="${MC_ALIAS:-local}"
ROOT_USER="${MARKHAND_MINIO_ROOT_USER:?MARKHAND_MINIO_ROOT_USER required}"
ROOT_PASSWORD="${MARKHAND_MINIO_ROOT_PASSWORD:?MARKHAND_MINIO_ROOT_PASSWORD required}"
APP_ACCESS_KEY="${MARKHAND_MINIO_ACCESS_KEY:?MARKHAND_MINIO_ACCESS_KEY required}"
APP_SECRET_KEY="${MARKHAND_MINIO_SECRET_KEY:?MARKHAND_MINIO_SECRET_KEY required}"
BUCKET="${MARKHAND_MINIO_BUCKET:-markhand-documents}"
POLICY_NAME="${MARKHAND_MINIO_POLICY_NAME:-markhand-app}"
POLICY_TMPL="${MARKHAND_MINIO_POLICY_TMPL:-/policies/minio-app-policy.json.tmpl}"

case "$BUCKET" in
  "" | *[!a-zA-Z0-9.-]*)
    echo "invalid MARKHAND_MINIO_BUCKET: ${BUCKET}" >&2
    exit 1
    ;;
esac

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

POLICY_FILE="$(mktemp)"
trap 'rm -f "$POLICY_FILE"' EXIT
sed "s/__BUCKET__/${BUCKET}/g" "$POLICY_TMPL" >"$POLICY_FILE"
grep -q "arn:aws:s3:::${BUCKET}" "$POLICY_FILE"

if mc admin policy info "$MC_ALIAS" "$POLICY_NAME" >/dev/null 2>&1; then
  mc admin policy remove "$MC_ALIAS" "$POLICY_NAME" >/dev/null 2>&1 \
    || mc admin policy rm "$MC_ALIAS" "$POLICY_NAME" >/dev/null 2>&1 \
    || true
fi

if mc admin policy create "$MC_ALIAS" "$POLICY_NAME" "$POLICY_FILE"; then
  :
elif mc admin policy add "$MC_ALIAS" "$POLICY_NAME" "$POLICY_FILE"; then
  :
else
  echo "failed to install MinIO policy ${POLICY_NAME}" >&2
  exit 1
fi

if ! mc admin user info "$MC_ALIAS" "$APP_ACCESS_KEY" >/dev/null 2>&1; then
  mc admin user add "$MC_ALIAS" "$APP_ACCESS_KEY" "$APP_SECRET_KEY"
fi

if mc admin policy attach "$MC_ALIAS" "$POLICY_NAME" --user "$APP_ACCESS_KEY"; then
  :
elif mc admin policy set "$MC_ALIAS" "$POLICY_NAME" "user=${APP_ACCESS_KEY}"; then
  :
else
  echo "failed to attach MinIO policy ${POLICY_NAME} to ${APP_ACCESS_KEY}" >&2
  exit 1
fi

echo "minio-init complete: bucket=${BUCKET} app_user=${APP_ACCESS_KEY} policy=${POLICY_NAME}"
