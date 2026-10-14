#!/usr/bin/env bash
# Deploy the DAT401 multi-Region blog demo. Idempotent: safe to re-run; every
# step either checks first or is a no-op when nothing changed.
#
#   ./deploy.sh
#
# Environment:
#   REGION_A (us-west-2)  REGION_B (us-east-1)  WITNESS_REGION (us-east-2)
#   SETUP_ARGS="..."      extra flags for db/setup.ts, e.g. "--skip-seed" or "--regrant"
#   SKIP_BOOTSTRAP=1      skip `cdk bootstrap` (it's idempotent, just slow-ish)
set -euo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INFRA_DIR="$DEMO_DIR/infra"
OUT_DIR="$DEMO_DIR/.deploy"
OUTPUTS="$DEMO_DIR/outputs.json"

REGION_A="${REGION_A:-us-west-2}"
REGION_B="${REGION_B:-us-east-1}"
WITNESS_REGION="${WITNESS_REGION:-us-east-2}"
SKIP_BOOTSTRAP="${SKIP_BOOTSTRAP:-0}"

# Fixed names (must match infra/lib/names.ts).
STACK_CLUSTER_A="Dat401-Cluster-$REGION_A"
STACK_CLUSTER_B="Dat401-Cluster-$REGION_B"
STACK_API_A="Dat401-Api-$REGION_A"
STACK_API_B="Dat401-Api-$REGION_B"
STACK_WEB="Dat401-Web"
STACK_DASHBOARD="Dat401-Dashboard"

export AWS_PAGER=""

log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
info() { printf '    %s\n' "$*"; }
die() {
  printf '\nerror: %s\n' "$*" >&2
  exit 1
}

# ---------------------------------------------------------------------------
log "1/8 Checking prerequisites"

for cmd in node npm aws jq curl; do
  command -v "$cmd" >/dev/null 2>&1 || die "$cmd is required but not on PATH"
done
node -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit(a>22||(a===22&&b>=12)?0:1)' ||
  die "Node.js >= 22.12 is required (found $(node --version))"
aws dsql help >/dev/null 2>&1 || die "this AWS CLI has no 'dsql' commands; install a current AWS CLI v2"
if [[ "$REGION_A" == "$REGION_B" || "$WITNESS_REGION" == "$REGION_A" || "$WITNESS_REGION" == "$REGION_B" ]]; then
  die "REGION_A, REGION_B and WITNESS_REGION must all differ"
fi

ACCOUNT="$(aws sts get-caller-identity --query Account --output text)" || die "no working AWS credentials"
info "account $ACCOUNT, Regions $REGION_A + $REGION_B, witness $WITNESS_REGION"

mkdir -p "$OUT_DIR"
(cd "$DEMO_DIR" && npm install --no-audit --no-fund --loglevel=error)

# Overridable only to dry-run this script with stubs.
CDK="${CDK:-$DEMO_DIR/node_modules/.bin/cdk}"
TSX="${TSX:-$DEMO_DIR/node_modules/.bin/tsx}"
[[ -x "$CDK" ]] || die "CDK CLI missing after npm install ($CDK)"

# All cdk calls run from infra/ with the Region context.
cdk() {
  (cd "$INFRA_DIR" && "$CDK" "$@" \
    -c "regionA=$REGION_A" -c "regionB=$REGION_B" -c "witnessRegion=$WITNESS_REGION")
}
deploy() { cdk deploy --require-approval never --exclusively --progress events "$@"; }

# stack_output <region> <stack> <output key>  ->  value, or "" if missing
stack_output() {
  local v
  v="$(aws cloudformation describe-stacks --region "$1" --stack-name "$2" \
    --query "Stacks[0].Outputs[?OutputKey=='$3'].OutputValue | [0]" --output text 2>/dev/null || true)"
  [[ "$v" == "None" ]] && v=""
  printf '%s' "$v"
}

# out <outputs file> <stack> <key>  ->  value from a cdk --outputs-file
out() {
  local v
  v="$(jq -r --arg s "$2" --arg k "$3" '.[$s][$k] // empty' "$1")"
  [[ -n "$v" ]] || die "output $3 missing for $2 in $1"
  printf '%s' "$v"
}

# wait_cluster_active <region> <cluster id>
wait_cluster_active() {
  local status
  for _ in $(seq 1 30); do
    status="$(aws dsql get-cluster --region "$1" --identifier "$2" --query status --output text)"
    case "$status" in
      ACTIVE) return 0 ;;
      FAILED | DELETING | DELETED | PENDING_DELETE) die "cluster $2 in $1 is $status" ;;
    esac
    info "$1 $2: $status"
    # The waiter polls every 2 s for up to 2 minutes; loop it for longer waits.
    aws dsql wait cluster-active --region "$1" --identifier "$2" 2>/dev/null && return 0
  done
  die "timed out waiting for cluster $2 in $1 to become ACTIVE"
}

# ---------------------------------------------------------------------------
log "2/8 Bootstrapping CDK in $REGION_A and $REGION_B"

if [[ "$SKIP_BOOTSTRAP" == "1" ]]; then
  info "skipped (SKIP_BOOTSTRAP=1)"
else
  cdk bootstrap "aws://$ACCOUNT/$REGION_A" "aws://$ACCOUNT/$REGION_B" -c clusterPhase=create
fi

# ---------------------------------------------------------------------------
log "3/8 Multi-Region Aurora DSQL clusters"

ARN_A="$(stack_output "$REGION_A" "$STACK_CLUSTER_A" ClusterArn)"
ARN_B="$(stack_output "$REGION_B" "$STACK_CLUSTER_B" ClusterArn)"

missing=()
if [[ -z "$ARN_A" ]]; then missing+=("$STACK_CLUSTER_A"); fi
if [[ -z "$ARN_B" ]]; then missing+=("$STACK_CLUSTER_B"); fi

if ((${#missing[@]} > 0)); then
  info "phase 1: creating ${missing[*]} with only the witness Region (they wait in PENDING_SETUP)"
  deploy --concurrency 2 -c clusterPhase=create --outputs-file "$OUT_DIR/clusters-phase1.json" "${missing[@]}"
  if [[ -z "$ARN_A" ]]; then ARN_A="$(out "$OUT_DIR/clusters-phase1.json" "$STACK_CLUSTER_A" ClusterArn)"; fi
  if [[ -z "$ARN_B" ]]; then ARN_B="$(out "$OUT_DIR/clusters-phase1.json" "$STACK_CLUSTER_B" ClusterArn)"; fi
else
  info "phase 1: both cluster stacks exist"
fi
CLUSTER_ARNS="$ARN_A,$ARN_B"
info "$ARN_A"
info "$ARN_B"

info "phase 2: peering (both stacks updated concurrently)"
deploy --concurrency 2 -c "clusterArns=$CLUSTER_ARNS" --outputs-file "$OUT_DIR/clusters.json" \
  "$STACK_CLUSTER_A" "$STACK_CLUSTER_B"

CLUSTER_ID_A="$(out "$OUT_DIR/clusters.json" "$STACK_CLUSTER_A" ClusterId)"
CLUSTER_ID_B="$(out "$OUT_DIR/clusters.json" "$STACK_CLUSTER_B" ClusterId)"
ENDPOINT_A="$(out "$OUT_DIR/clusters.json" "$STACK_CLUSTER_A" ClusterEndpoint)"
ENDPOINT_B="$(out "$OUT_DIR/clusters.json" "$STACK_CLUSTER_B" ClusterEndpoint)"

wait_cluster_active "$REGION_A" "$CLUSTER_ID_A"
wait_cluster_active "$REGION_B" "$CLUSTER_ID_B"
info "both clusters ACTIVE"

# ---------------------------------------------------------------------------
log "4/8 API (Lambda + function URL) in both Regions"

deploy --concurrency 2 -c "clusterArns=$CLUSTER_ARNS" --outputs-file "$OUT_DIR/api.json" "$STACK_API_A" "$STACK_API_B"
API_URL_A="$(out "$OUT_DIR/api.json" "$STACK_API_A" ApiUrl)"
API_URL_B="$(out "$OUT_DIR/api.json" "$STACK_API_B" ApiUrl)"
ROLE_ARN_A="$(out "$OUT_DIR/api.json" "$STACK_API_A" RoleArn)"
ROLE_ARN_B="$(out "$OUT_DIR/api.json" "$STACK_API_B" RoleArn)"
FUNCTION_A="$(out "$OUT_DIR/api.json" "$STACK_API_A" FunctionName)"
FUNCTION_B="$(out "$OUT_DIR/api.json" "$STACK_API_B" FunctionName)"

# ---------------------------------------------------------------------------
log "5/8 Database: schema, async indexes, blog_app role, IAM mappings, seed data"

# One logical database across the peered clusters, so this runs once, in REGION_A.
read -r -a setup_extra <<<"${SETUP_ARGS:-}"
(cd "$DEMO_DIR" && "$TSX" db/setup.ts \
  --endpoint "$ENDPOINT_A" --region "$REGION_A" \
  --role-arn "$ROLE_ARN_A" --role-arn "$ROLE_ARN_B" \
  ${setup_extra[@]+"${setup_extra[@]}"})

# ---------------------------------------------------------------------------
log "6/8 Web UI (S3 + CloudFront)"

(cd "$DEMO_DIR" && npm run build -w @dat401/web --silent)
deploy -c "clusterArns=$CLUSTER_ARNS" -c "apiUrls=$REGION_A=$API_URL_A,$REGION_B=$API_URL_B" \
  --outputs-file "$OUT_DIR/web.json" "$STACK_WEB"
WEB_URL="$(out "$OUT_DIR/web.json" "$STACK_WEB" WebUrl)"

# ---------------------------------------------------------------------------
log "7/8 CloudWatch dashboard"

deploy -c "clusterArns=$CLUSTER_ARNS" --outputs-file "$OUT_DIR/dashboard.json" "$STACK_DASHBOARD"
DASHBOARD_URL="$(out "$OUT_DIR/dashboard.json" "$STACK_DASHBOARD" DashboardUrl)"

# ---------------------------------------------------------------------------
log "8/8 Writing $OUTPUTS"

jq -n \
  --arg witness "$WITNESS_REGION" \
  --arg rA "$REGION_A" --arg urlA "$API_URL_A" --arg idA "$CLUSTER_ID_A" --arg arnA "$ARN_A" \
  --arg epA "$ENDPOINT_A" --arg fnA "$FUNCTION_A" \
  --arg rB "$REGION_B" --arg urlB "$API_URL_B" --arg idB "$CLUSTER_ID_B" --arg arnB "$ARN_B" \
  --arg epB "$ENDPOINT_B" --arg fnB "$FUNCTION_B" \
  --arg web "$WEB_URL" --arg dash "$DASHBOARD_URL" \
  '{
    witnessRegion: $witness,
    regions: [
      {region: $rA, apiUrl: $urlA, clusterId: $idA, clusterArn: $arnA, clusterEndpoint: $epA, functionName: $fnA},
      {region: $rB, apiUrl: $urlB, clusterId: $idB, clusterArn: $arnB, clusterEndpoint: $epB, functionName: $fnB}
    ],
    webUrl: $web,
    dashboardUrl: $dash
  }' >"$OUTPUTS"

# Smoke test: each Region's API reaches its own cluster endpoint.
health() {
  local body
  body="$(curl -sS --max-time 20 "${1}health" 2>&1 || true)"
  printf '%s' "$body" | jq -c '{ok, region, error, dbMs: .timing.dbMs}' 2>/dev/null || printf '%s' "$body"
}

cat <<EOF

DAT401 demo deployed.

  Web UI      $WEB_URL
  Dashboard   $DASHBOARD_URL

  $REGION_A   $API_URL_A
              cluster $CLUSTER_ID_A ($ENDPOINT_A)
              health  $(health "$API_URL_A")
  $REGION_B   $API_URL_B
              cluster $CLUSTER_ID_B ($ENDPOINT_B)
              health  $(health "$API_URL_B")

  Outputs     $OUTPUTS
EOF
