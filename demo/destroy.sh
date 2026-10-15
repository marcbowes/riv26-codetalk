#!/usr/bin/env bash
# Tear down the DAT401 demo: every stack in reverse deploy order. Deletes both
# clusters and all their data. Asks first unless FORCE=1.
#
#   ./destroy.sh
#   FORCE=1 ./destroy.sh
#
# Leaves the CDK bootstrap stacks (CDKToolkit) in place.
set -euo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGION_A="${REGION_A:-us-west-2}"
REGION_B="${REGION_B:-us-east-1}"
FORCE="${FORCE:-0}"

STACK_CLUSTER_A="Dat401-Cluster-$REGION_A"
STACK_CLUSTER_B="Dat401-Cluster-$REGION_B"
STACK_API_A="Dat401-Api-$REGION_A"
STACK_API_B="Dat401-Api-$REGION_B"
STACK_WEB="Dat401-Web"
STACK_DASHBOARD="Dat401-Dashboard"
STACK_LOADGEN="Dat401-Loadgen"

export AWS_PAGER=""

log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
info() { printf '    %s\n' "$*"; }
die() {
  printf '\nerror: %s\n' "$*" >&2
  exit 1
}

for cmd in aws jq; do
  command -v "$cmd" >/dev/null 2>&1 || die "$cmd is required but not on PATH"
done
ACCOUNT="$(aws sts get-caller-identity --query Account --output text)" || die "no working AWS credentials"

stack_output() {
  local v
  v="$(aws cloudformation describe-stacks --region "$1" --stack-name "$2" \
    --query "Stacks[0].Outputs[?OutputKey=='$3'].OutputValue | [0]" --output text 2>/dev/null || true)"
  [[ "$v" == "None" ]] && v=""
  printf '%s' "$v"
}

stack_exists() {
  aws cloudformation describe-stacks --region "$1" --stack-name "$2" >/dev/null 2>&1
}

CLUSTER_ID_A="$(stack_output "$REGION_A" "$STACK_CLUSTER_A" ClusterId)"
CLUSTER_ID_B="$(stack_output "$REGION_B" "$STACK_CLUSTER_B" ClusterId)"

if [[ "$FORCE" != "1" ]]; then
  cat <<EOF
This permanently deletes the DAT401 demo in account $ACCOUNT:
  clusters ${CLUSTER_ID_A:-?} ($REGION_A) and ${CLUSTER_ID_B:-?} ($REGION_B), with all their data,
  both APIs, the web UI, the dashboard and the load generator host.
EOF
  read -r -p "Type 'destroy' to continue: " answer
  [[ "$answer" == "destroy" ]] || die "aborted"
fi

# ---------------------------------------------------------------------------
log "Deletion protection"

for pair in "$REGION_A:$CLUSTER_ID_A" "$REGION_B:$CLUSTER_ID_B"; do
  region="${pair%%:*}"
  cluster="${pair#*:}"
  [[ -n "$cluster" ]] || continue
  protected="$(aws dsql get-cluster --region "$region" --identifier "$cluster" \
    --query deletionProtectionEnabled --output text 2>/dev/null || echo missing)"
  case "$protected" in
    True | true)
      info "disabling on $cluster ($region)"
      aws dsql update-cluster --region "$region" --identifier "$cluster" --no-deletion-protection-enabled >/dev/null
      aws dsql wait cluster-active --region "$region" --identifier "$cluster" 2>/dev/null || true
      ;;
    missing) info "$cluster ($region) not found" ;;
    *) info "already off on $cluster ($region)" ;;
  esac
done

# ---------------------------------------------------------------------------
# delete_stacks <region:stack>... : delete concurrently, wait for all
delete_stacks() {
  local pair region stack
  local started=()
  for pair in "$@"; do
    region="${pair%%:*}"
    stack="${pair#*:}"
    if stack_exists "$region" "$stack"; then
      info "deleting $stack ($region)"
      aws cloudformation delete-stack --region "$region" --stack-name "$stack"
      started+=("$pair")
    else
      info "$stack ($region): not deployed"
    fi
  done
  for pair in ${started[@]+"${started[@]}"}; do
    region="${pair%%:*}"
    stack="${pair#*:}"
    aws cloudformation wait stack-delete-complete --region "$region" --stack-name "$stack" ||
      die "$stack ($region) did not delete cleanly; see the CloudFormation console"
    info "$stack ($region): deleted"
  done
}

log "Stacks (reverse deploy order)"
delete_stacks "$REGION_A:$STACK_LOADGEN" "$REGION_A:$STACK_DASHBOARD" "$REGION_A:$STACK_WEB"
delete_stacks "$REGION_A:$STACK_API_A" "$REGION_B:$STACK_API_B"
# Together: a peered cluster sits in PENDING_DELETE until its peer is deleted too.
delete_stacks "$REGION_A:$STACK_CLUSTER_A" "$REGION_B:$STACK_CLUSTER_B"

rm -f "$DEMO_DIR/outputs.json"
rm -rf "$DEMO_DIR/.deploy"
log "Done. The CDKToolkit bootstrap stacks are left in place."
