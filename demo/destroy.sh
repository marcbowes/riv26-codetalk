#!/usr/bin/env bash
# Tear down the DAT401 demo: CDC streams, then every stack in reverse deploy
# order. Deletes both clusters and all their data. Asks first unless FORCE=1.
#
#   ./destroy.sh
#   FORCE=1 ./destroy.sh
#
# Leaves the CDK bootstrap stacks (CDKToolkit) in place.
set -euo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGION_A="${REGION_A:-us-east-1}"
REGION_B="${REGION_B:-us-east-2}"
FORCE="${FORCE:-0}"

STACK_CLUSTER_A="Dat401-Cluster-$REGION_A"
STACK_CLUSTER_B="Dat401-Cluster-$REGION_B"
STACK_API_A="Dat401-Api-$REGION_A"
STACK_API_B="Dat401-Api-$REGION_B"
STACK_EVENTS="Dat401-Events"
STACK_WEB="Dat401-Web"
STACK_DASHBOARD="Dat401-Dashboard"

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
  both APIs, the web UI, the dashboard, and the Kinesis streams dat401-cdc / dat401-demo-cdc.
EOF
  read -r -p "Type 'destroy' to continue: " answer
  [[ "$answer" == "destroy" ]] || die "aborted"
fi

# ---------------------------------------------------------------------------
log "CDC streams"

OUR_STREAMS=(
  "arn:aws:kinesis:$REGION_A:$ACCOUNT:stream/dat401-cdc"
  "arn:aws:kinesis:$REGION_A:$ACCOUNT:stream/dat401-demo-cdc"
)
is_ours() {
  local s
  for s in "${OUR_STREAMS[@]}"; do [[ "$1" == "$s" ]] && return 0; done
  return 1
}

# streams_into_ours <region> <cluster id>  ->  "streamId" lines targeting our Kinesis streams
streams_into_ours() {
  local sid target
  for sid in $(aws dsql list-streams --region "$1" --cluster-identifier "$2" \
    --query 'streams[?status!=`DELETING` && status!=`DELETED`].streamIdentifier' --output text 2>/dev/null || true); do
    target="$(aws dsql get-stream --region "$1" --cluster-identifier "$2" --stream-identifier "$sid" \
      --query 'targetDefinition.kinesis.streamArn' --output text 2>/dev/null || true)"
    if is_ours "$target"; then printf '%s\n' "$sid"; fi
  done
}

# The demo clusters' streams into our Kinesis streams: delete them.
for pair in "$REGION_A:$CLUSTER_ID_A" "$REGION_B:$CLUSTER_ID_B"; do
  region="${pair%%:*}"
  cluster="${pair#*:}"
  [[ -n "$cluster" ]] || continue
  for sid in $(streams_into_ours "$region" "$cluster"); do
    info "deleting CDC stream $sid on $cluster ($region)"
    aws dsql delete-stream --region "$region" --cluster-identifier "$cluster" --stream-identifier "$sid" >/dev/null
    for _ in $(seq 1 10); do
      aws dsql wait stream-not-exists --region "$region" --cluster-identifier "$cluster" \
        --stream-identifier "$sid" 2>/dev/null && break
    done
  done
done

# Other clusters (e.g. the one built live in the talk) aren't ours to delete; just warn.
for cluster in $(aws dsql list-clusters --region "$REGION_A" --query 'clusters[].identifier' --output text 2>/dev/null || true); do
  [[ "$cluster" == "$CLUSTER_ID_A" ]] && continue
  for sid in $(streams_into_ours "$REGION_A" "$cluster"); do
    info "warning: cluster $cluster has CDC stream $sid into a Kinesis stream that is about to be deleted."
    info "         Delete it with: aws dsql delete-stream --region $REGION_A --cluster-identifier $cluster --stream-identifier $sid"
  done
done

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
delete_stacks "$REGION_A:$STACK_DASHBOARD" "$REGION_A:$STACK_WEB"
delete_stacks "$REGION_A:$STACK_EVENTS"
delete_stacks "$REGION_A:$STACK_API_A" "$REGION_B:$STACK_API_B"
# Together: a peered cluster sits in PENDING_DELETE until its peer is deleted too.
delete_stacks "$REGION_A:$STACK_CLUSTER_A" "$REGION_B:$STACK_CLUSTER_B"

rm -f "$DEMO_DIR/outputs.json"
rm -rf "$DEMO_DIR/.deploy"
log "Done. The CDKToolkit bootstrap stacks are left in place."
