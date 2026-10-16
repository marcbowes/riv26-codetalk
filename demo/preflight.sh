#!/usr/bin/env bash
# Readiness check for the talk. Run it ~30 minutes before going on stage:
#
#   ./preflight.sh                 # check everything (also warms both APIs)
#   ./preflight.sh --start-host    # ...and start the load generator host if it's stopped
#   ./preflight.sh --smoke         # ...and run a 15 s load test from the host
#
# Every line is ok / WARN / FAIL with what to do about it. Exits 1 if anything
# failed. Read-only apart from --start-host and --smoke, and the warm-up reads.
#
# Environment: CDC_STREAM (dat401-cdc: step 10's Kinesis stream, set up by hand),
#   LAMBDA_CONCURRENCY (10000)
set -uo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$DEMO_DIR")"
OUTPUTS="$DEMO_DIR/outputs.json"
CDC_STREAM="${CDC_STREAM:-dat401-cdc}"
LAMBDA_CONCURRENCY="${LAMBDA_CONCURRENCY:-10000}"
START_HOST=0
SMOKE=0
for arg in "$@"; do
  case "$arg" in
    --start-host) START_HOST=1 ;;
    --smoke) START_HOST=1 SMOKE=1 ;;
    *) echo "usage: $0 [--start-host] [--smoke]" >&2; exit 2 ;;
  esac
done
export AWS_PAGER=""

FAILS=0
WARNS=0
section() { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok() { printf '  \033[32mok\033[0m    %s\n' "$*"; }
warn() { printf '  \033[33mWARN\033[0m  %s\n' "$*"; WARNS=$((WARNS + 1)); }
fail() { printf '  \033[31mFAIL\033[0m  %s\n' "$*"; FAILS=$((FAILS + 1)); }
have() { command -v "$1" >/dev/null 2>&1; }
j() { jq -r "$1" "$OUTPUTS"; }

# ---------------------------------------------------------------------------
section "Laptop"

for tool in aws jq curl psql session-manager-plugin python3; do
  if have "$tool"; then ok "$tool"; else fail "$tool is not installed"; fi
done
have aws && have jq || { echo "aws and jq are required"; exit 1; }

if ACCOUNT="$(aws sts get-caller-identity --query Account --output text 2>/dev/null)"; then
  ok "AWS credentials: account $ACCOUNT${AWS_PROFILE:+ (profile $AWS_PROFILE)}"
else
  fail "no working AWS credentials (export AWS_PROFILE=bowes?)"
  exit 1
fi
if [[ -f "$OUTPUTS" ]]; then ok "demo/outputs.json"; else fail "demo/outputs.json missing: run demo/deploy.sh"; exit 1; fi

REGION_A="$(j '.regions[0].region')"
REGION_B="$(j '.regions[1].region')"
for bin in "$REPO_DIR/events/target/release/events" "$DEMO_DIR/loadgen/target/release/loadgen"; do
  name="${bin#"$REPO_DIR"/}"
  if [[ -x "$bin" ]]; then ok "$name built"; else fail "$name not built: cargo build --release in $(dirname "$(dirname "$(dirname "$name")")")"; fi
done

if have sntp; then
  offset="$(sntp time.aws.com 2>/dev/null | awk '{print $1}' | tail -1)"
  if [[ "$offset" =~ ^[+-]?[0-9.]+$ ]]; then
    if python3 -c "import sys; sys.exit(0 if abs(float('$offset')) < 0.05 else 1)"; then
      ok "clock offset ${offset}s (events' lag columns are accurate)"
    else
      warn "clock is off by ${offset}s: the last two lag columns in events will be off by that much (sudo sntp -sS time.aws.com)"
    fi
  fi
fi

# ---------------------------------------------------------------------------
section "Quotas"

for region in "$REGION_A" "$REGION_B"; do
  c="$(aws lambda get-account-settings --region "$region" --query 'AccountLimit.ConcurrentExecutions' --output text 2>/dev/null)"
  if [[ -z "$c" ]]; then
    fail "$region: couldn't read the Lambda concurrency quota"
  elif ((c >= LAMBDA_CONCURRENCY)); then
    ok "$region Lambda concurrency $c (request rate cap $((c * 10))/s)"
  else
    warn "$region Lambda concurrency $c: requests above $((c * 10))/s get 429s. ./quotas.sh, or --region-weights to send less here"
  fi
done

# ---------------------------------------------------------------------------
section "Aurora DSQL clusters"

for i in 0 1; do
  region="$(j ".regions[$i].region")"
  id="$(j ".regions[$i].clusterId")"
  info="$(aws dsql get-cluster --region "$region" --identifier "$id" \
    --query '[status, length(multiRegionProperties.clusters || `[]`), deletionProtectionEnabled]' --output text 2>&1)"
  read -r status peers protected <<<"$info"
  if [[ "$status" == "ACTIVE" && "$peers" == "2" ]]; then
    ok "$region $id ACTIVE, peered${protected:+, deletion protection $protected}"
  else
    fail "$region $id: $info"
  fi
done

# ---------------------------------------------------------------------------
section "Web and APIs (through CloudFront)"

WEB="$(j .webUrl)"
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$WEB/")"
if [[ "$code" == "200" ]]; then ok "web UI $WEB"; else fail "web UI $WEB returned $code"; fi
cfg="$(curl -s --max-time 10 "$WEB/config.json" | jq -r '[.regions[].region] | join(",")' 2>/dev/null)"
if [[ "$cfg" == "$REGION_A,$REGION_B" ]]; then ok "config.json lists $cfg"; else fail "config.json lists '$cfg'"; fi

for i in 0 1; do
  region="$(j ".regions[$i].region")"
  api="$(j ".regions[$i].apiUrl")"
  body="$(curl -s --max-time 20 "${api}health")"
  if [[ "$(jq -r '.ok // false' <<<"$body" 2>/dev/null)" == "true" ]]; then
    ok "$region /health ok (db $(jq -r .timing.dbMs <<<"$body") ms)"
  else
    fail "$region /health: ${body:0:200}"
  fi
  # Warm a few execution environments, so the first clicks on stage aren't cold starts.
  times=""
  for _ in 1 2 3 4 5 6; do
    times+="$(curl -s -o /dev/null -w '%{time_total}' --max-time 10 "${api}posts?limit=20") "
  done
  ok "$region warmed: /posts round trips ${times% }s"
  posts="$(curl -s --max-time 10 "${api}posts?limit=1" | jq '.posts | length' 2>/dev/null)"
  [[ "$posts" == "1" ]] || fail "$region /posts returned no posts (seed data missing? SETUP_ARGS to re-run setup)"
done

for i in 0 1; do
  region="$(j ".regions[$i].region")"
  fn="$(j ".regions[$i].functionName")"
  auth="$(aws lambda get-function-url-config --region "$region" --function-name "$fn" --query AuthType --output text 2>&1)"
  stmts="$(aws lambda get-policy --region "$region" --function-name "$fn" --query Policy --output text 2>/dev/null |
    jq '[.Statement[] | select(.Principal.Service == "cloudfront.amazonaws.com")] | length' 2>/dev/null)"
  if [[ "$auth" == "AWS_IAM" && "${stmts:-0}" == "2" ]]; then
    ok "$region function URL: AWS_IAM, CloudFront may invoke it"
  else
    fail "$region function URL auth=$auth, CloudFront statements=${stmts:-0} (re-run deploy.sh)"
  fi
done

dash="$(aws cloudwatch get-dashboard --region "$REGION_A" --dashboard-name DAT401-Blog --query DashboardName --output text 2>/dev/null)"
if [[ "$dash" == "DAT401-Blog" ]]; then ok "CloudWatch dashboard DAT401-Blog ($REGION_A)"; else fail "dashboard DAT401-Blog missing"; fi

# ---------------------------------------------------------------------------
section "Load generator host"

HOST="$(j '.loadgen.instanceId // empty')"
HOST_REGION="$(j '.loadgen.region // empty')"
if [[ -z "$HOST" ]]; then
  fail "no load generator host in outputs.json (deploy.sh with LOADGEN_HOST=1)"
else
  state="$(aws ec2 describe-instances --region "$HOST_REGION" --instance-ids "$HOST" \
    --query 'Reservations[0].Instances[0].State.Name' --output text 2>&1)"
  if [[ "$state" == "stopped" && "$START_HOST" == "1" ]]; then
    aws ec2 start-instances --region "$HOST_REGION" --instance-ids "$HOST" >/dev/null
    aws ec2 wait instance-running --region "$HOST_REGION" --instance-ids "$HOST"
    state=running
    ok "$HOST started"
  fi
  if [[ "$state" != "running" ]]; then
    fail "$HOST is $state (./preflight.sh --start-host, or any loadgen --run-on-ec2)"
  else
    ping=""
    for _ in $(seq 1 36); do
      ping="$(aws ssm describe-instance-information --region "$HOST_REGION" \
        --filters "Key=InstanceIds,Values=$HOST" --query 'InstanceInformationList[0].PingStatus' --output text)"
      [[ "$ping" == "Online" ]] && break
      sleep 5
    done
    if [[ "$ping" != "Online" ]]; then
      fail "$HOST is running but SSM is $ping"
    else
      cmd="$(aws ssm send-command --region "$HOST_REGION" --instance-ids "$HOST" --document-name AWS-RunShellScript \
        --parameters 'commands=["cut -d. -f1 /proc/uptime","test -f /home/ec2-user/READY && echo READY || echo NOT_READY","sudo -u ec2-user git -C /home/ec2-user/riv26-codetalk rev-parse HEAD 2>/dev/null || echo none","test -x /home/ec2-user/riv26-codetalk/demo/loadgen/target/release/loadgen && echo BUILT || echo NOT_BUILT"]' \
        --query Command.CommandId --output text)"
      out=""
      for _ in $(seq 1 20); do
        sleep 2
        res="$(aws ssm get-command-invocation --region "$HOST_REGION" --command-id "$cmd" --instance-id "$HOST" \
          --query '[Status,StandardOutputContent]' --output text 2>/dev/null)"
        [[ "$res" == Success* ]] && { out="${res#Success}"; break; }
      done
      read -r uptime ready head built <<<"$(tr '\n' ' ' <<<"$out")"
      if [[ -z "$out" ]]; then
        fail "$HOST didn't answer an SSM command"
      else
        [[ "$ready" == "READY" && "$built" == "BUILT" ]] && ok "$HOST setup done, loadgen built" ||
          fail "$HOST setup not finished ($ready, $built): /var/log/dat401-setup.log on the host"
        if ((uptime >= 900)); then
          ok "$HOST up $((uptime / 60)) min"
        else
          warn "$HOST only up $((uptime / 60)) min: a patch reboot may still be coming; give it until 15 min"
        fi
        main="$(git -C "$REPO_DIR" ls-remote origin refs/heads/main 2>/dev/null | cut -f1)"
        if [[ -n "$main" && "$head" == "$main" ]]; then
          ok "$HOST checkout is at origin/main ($(cut -c1-7 <<<"$main"))"
        else
          warn "$HOST checkout ${head:0:7} != origin/main ${main:0:7}: the next --run-on-ec2 pulls and rebuilds (can take a minute)"
        fi
      fi
    fi
  fi
fi

# ---------------------------------------------------------------------------
section "Patching (the account's AWS-RunPatchBaseline association reboots instances)"

for aid in $(aws ssm list-associations --region "$HOST_REGION" \
  --association-filter-list key=Name,value=AWS-RunPatchBaseline --query 'Associations[].AssociationId' --output text); do
  last="$(aws ssm describe-association-executions --region "$HOST_REGION" --association-id "$aid" --max-results 1 \
    --query 'AssociationExecutions[0].CreatedTime' --output text)"
  sched="$(aws ssm describe-association --region "$HOST_REGION" --association-id "$aid" \
    --query 'AssociationDescription.ScheduleExpression' --output text)"
  verdict="$(python3 - "$last" "$sched" <<'PY'
import sys, re, datetime as dt
last, sched = sys.argv[1], sys.argv[2]
m = re.match(r"rate\((\d+) days?\)", sched)
if not m or last in ("", "None"):
    print(f"unknown|schedule {sched}, last run {last}")
    sys.exit()
nxt = dt.datetime.fromisoformat(last) + dt.timedelta(days=int(m.group(1)))
hours = (nxt - dt.datetime.now(dt.timezone.utc)).total_seconds() / 3600
when = nxt.astimezone().strftime("%a %b %d %H:%M %Z")
print(("soon" if 0 <= hours < 3 else "ok") + f"|next fleet run {when} ({hours:.0f} h from now; {sched})")
PY
)"
  case "${verdict%%|*}" in
    ok) ok "${verdict#*|}" ;;
    soon) warn "${verdict#*|}: the host may patch and reboot during the talk" ;;
    *) warn "${verdict#*|}" ;;
  esac
done

# ---------------------------------------------------------------------------
section "Live demo (step 10)"

kin="$(aws kinesis describe-stream-summary --region "$REGION_A" --stream-name "$CDC_STREAM" \
  --query 'StreamDescriptionSummary.[StreamStatus,StreamModeDetails.StreamMode,MaxRecordSizeInKiB]' --output text 2>/dev/null)"
if [[ "$kin" == "ACTIVE"*"ON_DEMAND"*"10240" ]]; then
  ok "Kinesis stream $CDC_STREAM: ACTIVE, ON_DEMAND, 10 MiB records"
elif [[ -n "$kin" ]]; then
  warn "Kinesis stream $CDC_STREAM: $kin (CDC wants ON_DEMAND and MaxRecordSizeInKiB 10240)"
else
  others="$(aws kinesis list-streams --region "$REGION_A" --query 'StreamNames' --output text 2>/dev/null | tr '\t' ' ')"
  warn "no Kinesis stream $CDC_STREAM in $REGION_A for step 10 (CDC_STREAM=<name> ./preflight.sh; streams here: ${others:-none})"
fi

# ---------------------------------------------------------------------------
if ((SMOKE && FAILS == 0)); then
  section "Smoke test from the load generator host (15 s, 1,000 req/s)"
  (cd "$DEMO_DIR/loadgen" && script -q /dev/null ./target/release/loadgen --run-on-ec2 \
    --rps 1000 --ramp 5 --duration 15s --plain 2>&1 | tr -d '\r' | grep -E '^(Region|[a-z]{2}-[a-z]+-[0-9]|all) ')
fi

echo
if ((FAILS)); then
  printf '\033[31m%d failed\033[0m, %d warnings\n' "$FAILS" "$WARNS"
  exit 1
fi
printf '\033[32mReady\033[0m (%d warnings)\n' "$WARNS"
