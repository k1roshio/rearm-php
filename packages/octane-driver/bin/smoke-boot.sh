#!/bin/sh
set -eu
SOCK=/tmp/rearm-smoke.sock
rm -f "$SOCK"
export REARM_SOCK="$SOCK"
export REARM_APP_ROOT=/var/www/apps/bench
echo "starting worker..."
timeout 15 php /var/www/packages/octane-driver/bin/worker.php > /tmp/worker-out.log 2>&1 &
PID=$!
sleep 8
if kill -0 "$PID" 2>/dev/null; then
  echo "WORKER_ALIVE pid=$PID"
  ls -la "$SOCK" || true
  kill "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true
  exit 0
fi
echo "WORKER_DEAD"
cat /tmp/worker-out.log || true
exit 1
