#!/usr/bin/env bash
# Smoke test against a running dwi-api (default http://localhost:8080).
set -euo pipefail
B=${API:-http://localhost:8080}
j() { python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }

echo "health: $(curl -sf $B/healthz)"
T=$(curl -sf $B/v1/auth/login -H 'content-type: application/json' \
  -d '{"email":"admin@demo.local","password":"demo-password-123"}' | j 'd["token"]')
EV=$(curl -sf $B/v1/events -H "authorization: Bearer $T" | j 'd["events"][0]["id"]')
curl -sf $B/v1/events/$EV -H "authorization: Bearer $T" | j '" | ".join([d["event"]["coupleLine"],d["event"]["dateLabel"],d["event"]["venue"]])'
CODE=$(curl -sf -XPOST $B/v1/events/$EV/pairing-codes -H "authorization: Bearer $T" \
  -H 'content-type: application/json' -d '{"label":"smoke"}' | j 'd["code"]')
echo "pairing code: $CODE"
D=$(curl -sf $B/v1/devices/pair -H 'content-type: application/json' \
  -d "{\"code\":\"$CODE\",\"deviceName\":\"Pintu Utara\"}" | j 'd["deviceToken"]')
echo "device token: ${D:0:10}…"

curl -sN "$B/v1/events/$EV/live?access_token=$T" > /tmp/dwi-sse.txt &
SSE=$!
sleep 1
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
ID=$(uuidgen | tr 'A-Z' 'a-z')
BODY='{"deviceNow":"'$NOW'","checkIns":[{"id":"'$ID'","code":"AJ27-VIP-0006-6","headcount":2,"occurredAt":"'$NOW'","override":false}]}'
RES=$(curl -sf $B/v1/sync -H "authorization: Bearer $D" -H 'content-type: application/json' -d "$BODY")
echo "sync: $RES"
sleep 1
kill $SSE 2>/dev/null || true
echo "--- live stream received:"
cat /tmp/dwi-sse.txt
echo "--- CORS preflight from the scanner origin:"
curl -s -o /dev/null -D - -XOPTIONS $B/v1/sync -H 'origin: http://localhost:5173' \
  -H 'access-control-request-method: POST' | grep -i "^HTTP\|access-control-allow-origin"
echo "--- attendance:"
curl -sf $B/v1/events/$EV/attendance -H "authorization: Bearer $T" \
  | j '{k: d[k] for k in ["households","expectedPeople","checkedInHouseholds","checkedInPeople"]}'
