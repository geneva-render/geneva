#!/usr/bin/env bash
# The geneva image's entry point.
#
# - With arguments: runs geneva with them.
# - With none: runs `geneva worker`, which reads GENEVA_FARM_URL and
#   GENEVA_FARM_TOKEN.
# - On AWS Lambda (AWS_LAMBDA_RUNTIME_API is set): takes invocations
#   whose payload is {"url": ..., "token": ...} and runs a worker for
#   each. The worker takes no new part once two thirds of the time left
#   have passed (GENEVA_STOP_MARGIN, in seconds, sets the time kept free
#   instead), so the part it is rendering can finish before the limit.
set -euo pipefail

if [ -z "${AWS_LAMBDA_RUNTIME_API:-}" ]; then
  if [ $# -eq 0 ]; then
    exec geneva worker
  fi
  exec geneva "$@"
fi

api="http://$AWS_LAMBDA_RUNTIME_API/2018-06-01/runtime/invocation"
# Only /tmp is writable on Lambda.
export HOME=/tmp TMPDIR=/tmp
headers=/tmp/geneva-invocation-headers

header() {
  grep -i "^$1:" "$headers" | head -n1 | cut -d: -f2- | tr -d ' \r'
}

while true; do
  event=$(curl -sS -D "$headers" "$api/next")
  id=$(header Lambda-Runtime-Aws-Request-Id)
  deadline=$(header Lambda-Runtime-Deadline-Ms)
  url=$(jq -r '.url // empty' <<<"$event")
  token=$(jq -r '.token // empty' <<<"$event")
  if [ -z "$url" ] || [ -z "$token" ]; then
    jq -n '{errorType: "BadPayload", errorMessage: "the payload needs url and token"}' |
      curl -sS -X POST "$api/$id/error" -H "Lambda-Runtime-Function-Error-Type: Unhandled" --data-binary @- >/dev/null
    continue
  fi
  left=$(( (deadline - $(date +%s%3N)) / 1000 ))
  margin=${GENEVA_STOP_MARGIN:-$(( left / 3 ))}
  stop=$(( left - margin ))
  [ "$stop" -lt 0 ] && stop=0
  if out=$(geneva --format json worker --connect "$url" --token "$token" \
             --name "lambda-${id:0:8}" --stop-after "$stop"); then
    printf '%s' "$out" |
      curl -sS -X POST "$api/$id/response" --data-binary @- >/dev/null
  else
    jq -n --arg out "$out" '{errorType: "WorkerFailed", errorMessage: $out}' |
      curl -sS -X POST "$api/$id/error" -H "Lambda-Runtime-Function-Error-Type: Unhandled" --data-binary @- >/dev/null
  fi
done
