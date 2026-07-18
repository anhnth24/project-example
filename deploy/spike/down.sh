#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
"${COMPOSE[@]}" --profile gpu down --remove-orphans
