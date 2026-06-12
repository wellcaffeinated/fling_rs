#!/bin/sh
# Smoke test exercised from the client container: relays commands to the server
# container over the shared Unix socket and asserts the access rules hold.
set -u

fail=0

note() { echo "[smoke] $*"; }
pass() { echo "  PASS: $*"; }
bad()  { echo "  FAIL: $*"; fail=1; }

# Wait for the server: `foo status` is an allowed command, so a clean exit means
# the server is up and answering. Connection failures exit non-zero and retry.
note "waiting for server on ${FLING_SOCKET:-unix:/run/fling/fling.sock} ..."
i=0
until foo status >/dev/null 2>&1; do
    i=$((i + 1))
    if [ "$i" -gt 100 ]; then
        note "server never became ready"
        exit 1
    fi
    sleep 0.1
done
note "server is up"

# 1. Allowed subcommand, output relayed verbatim.
out=$(foo list everything); rc=$?
if [ "$rc" -eq 0 ] && [ "$out" = "list everything" ]; then
    pass "foo list everything -> '$out'"
else
    bad "foo list everything (rc=$rc out='$out')"
fi

# 2. Disallowed subcommand denied by the glob rules.
err=$(foo create thing 2>&1 >/dev/null); rc=$?
if [ "$rc" -ne 0 ] && echo "$err" | grep -q "not permitted"; then
    pass "foo create thing denied (rc=$rc)"
else
    bad "foo create thing should be denied (rc=$rc err='$err')"
fi

# 3. Stdin relayed across containers through cat.
out=$(printf 'piped-through\n' | relaycat); rc=$?
if [ "$rc" -eq 0 ] && [ "$out" = "piped-through" ]; then
    pass "relaycat stdin relay -> '$out'"
else
    bad "relaycat stdin relay (rc=$rc out='$out')"
fi

# 4. Unconfigured command (symlink exists, but no config entry) is denied.
err=$(bar anything 2>&1 >/dev/null); rc=$?
if [ "$rc" -ne 0 ] && echo "$err" | grep -q "not configured"; then
    pass "bar anything denied (rc=$rc)"
else
    bad "bar anything should be denied (rc=$rc err='$err')"
fi

if [ "$fail" -eq 0 ]; then
    note "ALL SMOKE TESTS PASSED"
else
    note "SMOKE TESTS FAILED"
fi
exit "$fail"
