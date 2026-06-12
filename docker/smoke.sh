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

# 2. Disallowed subcommand denied by the glob rules, with the uniform message.
err=$(foo create thing 2>&1 >/dev/null); rc=$?
if [ "$rc" -ne 0 ] && [ "$err" = "You are not authorized to execute this command" ]; then
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

# 4. Unconfigured command (symlink exists, but no config entry) is denied with
#    the same uniform message — the rules don't leak which commands exist.
err=$(bar anything 2>&1 >/dev/null); rc=$?
if [ "$rc" -ne 0 ] && [ "$err" = "You are not authorized to execute this command" ]; then
    pass "bar anything denied (rc=$rc)"
else
    bad "bar anything should be denied (rc=$rc err='$err')"
fi

# 5. Sandboxed command can read a file inside its bound directory.
out=$(safecat /data/public/hello.txt); rc=$?
if [ "$rc" -eq 0 ] && [ "$out" = "public data" ]; then
    pass "safecat reads bound /data/public (-> '$out')"
else
    bad "safecat should read /data/public/hello.txt (rc=$rc out='$out')"
fi

# 6. Sandbox blocks files outside the bound directory, even though the glob
#    rule (`*`) would permit the argument. /etc/passwd is simply not present.
out=$(safecat /etc/passwd 2>/dev/null); rc=$?
if [ "$rc" -ne 0 ] && [ -z "$out" ]; then
    pass "safecat cannot see /etc/passwd (rc=$rc)"
else
    bad "safecat should NOT read /etc/passwd (rc=$rc out='$out')"
fi

# 7. ... and likewise for a sibling file under the unbound /data parent.
out=$(safecat /data/secret.txt 2>/dev/null); rc=$?
if [ "$rc" -ne 0 ] && [ -z "$out" ]; then
    pass "safecat cannot see /data/secret.txt (rc=$rc)"
else
    bad "safecat should NOT read /data/secret.txt (rc=$rc out='$out')"
fi

if [ "$fail" -eq 0 ]; then
    note "ALL SMOKE TESTS PASSED"
else
    note "SMOKE TESTS FAILED"
fi
exit "$fail"
