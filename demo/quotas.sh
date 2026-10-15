#!/usr/bin/env bash
# Request the service quota increases the finale needs, in both demo Regions.
# Idempotent: skips quotas that are already high enough or have a request open.
#
#   ./quotas.sh            # request what's missing
#   ./quotas.sh --status   # just report
#
# Why Lambda concurrency: besides capping concurrent executions, it caps
# synchronous requests (function URLs included) at 10x per second. At the
# default 1,000 that's 10,000 req/s per Region, which the finale's load
# generator exceeds. 10,000 lifts the rate cap to 100,000 req/s.
#
# Environment: REGION_A (us-west-2), REGION_B (us-east-1),
#   LAMBDA_CONCURRENCY (10000)
set -euo pipefail

REGION_A="${REGION_A:-us-west-2}"
REGION_B="${REGION_B:-us-east-1}"
LAMBDA_CONCURRENCY="${LAMBDA_CONCURRENCY:-10000}"
STATUS_ONLY=0
[[ "${1:-}" == "--status" ]] && STATUS_ONLY=1

export AWS_PAGER=""

# quota <region> <service> <quota code> <name> <target>
quota() {
  local region=$1 service=$2 code=$3 name=$4 target=$5 applied open request
  applied="$(aws service-quotas get-service-quota --region "$region" --service-code "$service" \
    --quota-code "$code" --query 'Quota.Value' --output text 2>/dev/null ||
    aws service-quotas get-aws-default-service-quota --region "$region" --service-code "$service" \
      --quota-code "$code" --query 'Quota.Value' --output text)"
  applied="${applied%.0}"
  if ((applied >= target)); then
    printf '  ok       %-10s %-34s %s (want %s)\n' "$region" "$name" "$applied" "$target"
    return
  fi
  open="$(aws service-quotas list-requested-service-quota-change-history-by-quota --region "$region" \
    --service-code "$service" --quota-code "$code" \
    --query 'RequestedQuotas[?Status==`PENDING` || Status==`CASE_OPENED`] | [0].[Status,DesiredValue,Created]' \
    --output text)"
  if [[ -n "$open" && "$open" != "None" ]]; then
    printf '  pending  %-10s %-34s %s, requested: %s\n' "$region" "$name" "$applied" "$open"
    return
  fi
  if ((STATUS_ONLY)); then
    printf '  LOW      %-10s %-34s %s (want %s; run ./quotas.sh to request)\n' "$region" "$name" "$applied" "$target"
    return
  fi
  request="$(aws service-quotas request-service-quota-increase --region "$region" --service-code "$service" \
    --quota-code "$code" --desired-value "$target" --query 'RequestedQuota.[Id,Status]' --output text)"
  printf '  REQUESTED %-9s %-34s %s -> %s: %s\n' "$region" "$name" "$applied" "$target" "$request"
}

echo "Account $(aws sts get-caller-identity --query Account --output text)"
for region in "$REGION_A" "$REGION_B"; do
  quota "$region" lambda L-B99A9384 "Lambda concurrent executions" "$LAMBDA_CONCURRENCY"
done
echo "Requests can take hours to days; re-run with --status to check."
