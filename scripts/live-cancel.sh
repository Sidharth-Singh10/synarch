#!/usr/bin/env bash
# Opt-in cancellation verification; consumes provider allowance.
set -euo pipefail
if [[ "${SYNARCH_LIVE_CONFIRM:-}" != "1" ]]; then
    echo "refusing to spend provider credits: set SYNARCH_LIVE_CONFIRM=1" >&2
    exit 2
fi
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }
command -v base64 >/dev/null || { echo "base64 is required" >&2; exit 2; }
state_dir="$(opencode debug paths state)"
registration="${state_dir}/service.json"
[[ -f "$registration" ]] || { echo "OpenCode service registration not found" >&2; exit 2; }
readarray -t connection < <(python3 - "$registration" <<'PY'
import json, sys
registration = json.load(open(sys.argv[1], encoding="utf-8"))
print(registration["url"])
print(registration["password"])
PY
)
export SYNARCH_OPENCODE_URL="${connection[0]}"
export SYNARCH_OPENCODE_AUTHORIZATION="Basic $(printf 'opencode:%s' "${connection[1]}" | base64 -w0)"
unset connection
echo "live cancellation harness: model ${SYNARCH_LIVE_MODEL:-opencode-go/deepseek-v4.1-flash}"
cargo test --locked --test live live_interrupts_actively_generating_worker -- --ignored --nocapture --test-threads=1 "$@"
