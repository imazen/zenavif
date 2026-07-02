#!/usr/bin/env bash
# Show sweep-box state, uptime, and accrued cost. Loudly warns when the box has
# been up >12h — an idle box left running is a defect, not a convenience.
source "$(dirname "$0")/common.sh"
load_token

if ! "$HCLOUD" server describe "$BOX_NAME" >/dev/null 2>&1; then
  echo "[status] no '$BOX_NAME' box exists — cost EUR 0/h. (provision.sh creates one)"
  exit 0
fi

# NOTE: the python block is inside shell single quotes — inner strings must be
# double-quoted (a single quote in here would terminate the shell argument).
"$HCLOUD" server describe "$BOX_NAME" -o json | python3 -c '
import json, sys, datetime
d = json.load(sys.stdin)
# .replace: python<3.11 fromisoformat cannot parse a trailing Z
created = datetime.datetime.fromisoformat(d["created"].replace("Z", "+00:00"))
up_h = (datetime.datetime.now(datetime.timezone.utc) - created).total_seconds() / 3600
# current hcloud emits a top-level "location"; "datacenter" can be null
loc = (d.get("location") or (d.get("datacenter") or {}).get("location") or {}).get("name", "?")
prices = [p for p in d["server_type"]["prices"] if p["location"] == loc]
hourly = float(prices[0]["price_hourly"]["gross"]) if prices else float("nan")
name, status = d["name"], d["status"]
stype, cores = d["server_type"]["name"], d["server_type"]["cores"]
ip = d["public_net"]["ipv4"]["ip"]
print(f"[status] {name}: {status}  type={stype} ({cores} cores)  loc={loc}  ip={ip}")
print(f"[status] up {up_h:.1f} h   ~EUR {hourly:.3f}/h gross   accrued this uptime ~EUR {up_h*hourly:.2f}")
if up_h > 12:
    print("!" * 78)
    print(f"!! WARNING: box has been up {up_h:.1f} h (>12 h).")
    print( "!! If no sweep is running, tear it down NOW:  ./teardown.sh --yes")
    print("!" * 78)
'

# Best-effort live view (5s timeout; box may be mid-reboot).
require_box_ip
if out=$(ssh "${SSH_OPTS[@]}" -o ConnectTimeout=5 "root@$BOX_IP" '
  echo "load: $(cut -d" " -f1-3 /proc/loadavg)  ($(nproc) cores)"
  sweeps=$(pgrep -fc "run_gap.sh|aom_only.sh|palette_ablation.sh|tool_ablation.sh" 2>/dev/null || true)
  encs=$(pgrep -c "aomenc|cavif" 2>/dev/null || true)
  echo "sweep-related procs (drivers+workers): ${sweeps:-0}   encoder procs: ${encs:-0}"
  echo "unfetched run dirs in /home/lilith/sweep_out:"
  ls -1t /home/lilith/sweep_out 2>/dev/null | head -5 | sed "s/^/  /" || true
' 2>/dev/null); then
  echo "$out" | sed 's/^/[status] /'
else
  echo "[status] (ssh probe failed — box unreachable right now)"
fi
