#!/usr/bin/env bash
# The client's side of the Docker suite.
#
# Everything asserted here is something the in-process tests cannot prove: that
# the SSH bootstrap works against a real sshd, that UDP reaches a second machine,
# and that the session genuinely runs *there* rather than here.
set -uo pipefail

pass=0
fail=0

# Assert `expected` appears in `output`, and that the command did not error.
#
# The error check is not belt-and-braces: a first draft of this suite "passed"
# its hostname test because the failure message *also* contained the hostname it
# was grepping for. An assertion that a broken run can satisfy is worse than no
# assertion, so anything that looks like a failure fails the check outright.
check() {
  local what="$1" expected="$2" output="$3"
  if grep -qE '^Error:|error:' <<<"$output"; then
    printf '  FAIL  %s\n         the command reported an error:\n' "$what"
    printf '%s\n' "$output" | sed 's/^/         | /'
    fail=$((fail + 1))
    return
  fi
  if grep -qF -- "$expected" <<<"$output"; then
    printf '  PASS  %s\n' "$what"
    pass=$((pass + 1))
  else
    printf '  FAIL  %s\n         expected to find: %s\n' "$what" "$expected"
    printf '         got:\n'
    printf '%s\n' "$output" | sed 's/^/         | /'
    fail=$((fail + 1))
  fi
}

# Assert a command fails, and says something useful about why.
check_error() {
  local what="$1" expected="$2" output="$3"
  if grep -qF -- "$expected" <<<"$output"; then
    printf '  PASS  %s\n' "$what"
    pass=$((pass + 1))
  else
    printf '  FAIL  %s\n         expected to find: %s\n         got:\n' "$what" "$expected"
    printf '%s\n' "$output" | sed 's/^/         | /'
    fail=$((fail + 1))
  fi
}

echo "waiting for sshd on host…"
for _ in $(seq 1 60); do
  if ssh -o ConnectTimeout=2 root@host true 2>/dev/null; then break; fi
  sleep 1
done
ssh -o ConnectTimeout=2 root@host true || { echo "FATAL: sshd never came up"; exit 1; }
echo "sshd is up"
echo

# 0. Where we are, so a failure below can be read against it.
echo "this container is $(hostname)"
echo

# 1. The session runs on the other machine.
#
#    Tagged rather than bare: a plain `buildbox` would also match the text of
#    several *failure* messages, and an assertion a broken run can satisfy is
#    worse than none. The tag can only come from a shell that really ran there.
out=$(medulla remote buildbox --exec 'echo "ran-on=$(hostname)"' 2>&1)
check "the session runs on the remote machine" "ran-on=buildbox" "$out"
if grep -qF "ran-on=$(hostname)" <<<"$out"; then
  echo "  FAIL  the session ran locally — that is not a remote session at all"
  fail=$((fail + 1))
fi

# 2. It runs in the remote filesystem. This file is mounted only into the host
#    container, so reading it proves the pty is over there.
out=$(medulla remote buildbox --exec 'cat /work/marker.txt' 2>&1)
check "the remote filesystem is what the session sees" "this file only exists on the remote host" "$out"

# 3. The workspace from [[remoteHosts]] is honoured.
out=$(medulla remote buildbox --exec 'pwd' 2>&1)
check "the configured workspace is where the session starts" "/work" "$out"

# 4. Output larger than one datagram still arrives, which is the screen
#    protocol's chunking and reassembly working over a real MTU.
out=$(medulla remote buildbox --exec 'seq 1 200 | tr "\n" " "' 2>&1)
check "a screenful of output survives the wire" "195 196 197 198 199 200" "$out"

# 4b. Output long enough to scroll the command line off the 40-row screen. The
#     completion marker has to be detectable when only the *output* copy is
#     still visible — counting two occurrences fails here, and reported a
#     timeout for a command that had finished.
out=$(medulla remote buildbox --exec 'seq 1 300' 2>&1)
check "a command whose output scrolls past a screenful still completes" "300" "$out"

# 5. A bad host id fails clearly rather than hanging.
out=$(medulla remote nosuchhost --exec 'true' 2>&1)
check_error "an unknown host names the ones that are configured" \
  "no [[remoteHosts]] entry called nosuchhost" "$out"

# 6. One daemon per client, reused rather than duplicated. Four `medulla remote`
#    runs have happened by now; if each had started its own daemon there would be
#    four processes over there, and each would own a different set of sessions.
out=$(ssh root@host 'pgrep -c -f "medulla daemon --direct"' 2>&1)
check "the daemon is reused across reconnects, not duplicated" "1" "$out"

# 7. Sessions outlive the client that opened them. This is the point of the
#    transport: a client goes away and comes back to what it left. The session
#    below is opened by one process and found by the next.
ssh root@host 'ls /root/.medulla/local/remote/hosts' >/dev/null 2>&1
out=$(medulla remote buildbox --exec 'echo second-connection-works' 2>&1)
check "a later connection reaches the same daemon" "second-connection-works" "$out"

# ── Coding agents on remote hosts ───────────────────────────────────────────
#
# A shell proves the transport. These prove the thing a shell deliberately does
# *not* exercise: everything `open_unmanaged_named` does for a coding agent and
# skips for a shell. All of it has to happen on the far side, because that is
# where the process runs.

# 8. The host offers what it actually has, not what the client has.
out=$(medulla remote buildbox --harness claude --exec '' 2>&1)
check "a coding agent on the remote host starts at all" "AGENT-READY" "$out"

# 9. It runs over there, in the configured workspace.
check "the agent runs on the remote machine" "AGENT-HOST: buildbox" "$out"
check "the agent starts in the remote workspace" "AGENT-CWD: /work" "$out"

# 10. The grant. This is the one that would silently not work: `attach_mcp`
#     mints against the control socket of the process that launches the harness,
#     so without the daemon binding one, a remote agent would start with no way
#     to report and no tools — while a local one got both.
check "the agent is given a hook socket to report through" "AGENT-HOOK-SOCKET: /" "$out"
check "the agent is given a grant for it" "AGENT-HOOK-GRANT: <set>" "$out"

# 11. It is a live session, not a one-shot: input reaches it and it answers.
out=$(medulla remote buildbox --harness claude --exec 'hello agent' 2>&1)
check "the agent receives what is typed at it" "AGENT-ECHO: hello agent" "$out"

# 12. Naming a harness the host does not have fails by listing what it does.
out=$(medulla remote buildbox --harness nosuchagent --exec '' 2>&1)
check_error "an unavailable harness names what the host does offer" \
  "does not offer nosuchagent" "$out"

# 13. The daemon outlives the SSH session that started it — the property the
#     whole design rests on. Asserted three ways, because "it happened to still
#     be there" is not the same as "it detached".
out=$(ssh root@host 'ps -eo pid=,ppid=,sid=,args= | grep "[d]aemon --direct" | head -1' 2>&1)
read -r dpid dppid dsid _rest <<<"$out"
#     Reparented to init: sshd's child is gone, so nothing upstream can signal
#     it when a session ends.
if [ "${dppid:-0}" = "1" ]; then
  echo "  PASS  the daemon's parent is init, not sshd"
  pass=$((pass + 1))
else
  echo "  FAIL  the daemon still has an sshd parent (ppid=${dppid:-?}): $out"
  fail=$((fail + 1))
fi
#     And its own session leader, so it has no controlling terminal to be
#     hung up on.
if [ -n "${dpid:-}" ] && [ "${dsid:-}" = "${dpid:-}" ]; then
  echo "  PASS  the daemon leads its own session"
  pass=$((pass + 1))
else
  echo "  FAIL  the daemon is still in the ssh session (pid=${dpid:-?} sid=${dsid:-?})"
  fail=$((fail + 1))
fi
#     And no ssh client is left behind on this side.
# `pgrep -c` prints its count *and* exits nonzero when that count is zero, so a
# `|| echo 0` fallback appends a second line and the comparison breaks.
left=$(pgrep -c -f "ssh .*root@host" 2>/dev/null)
left=${left:-0}
if [ "$left" -eq 0 ]; then
  echo "  PASS  no orphaned ssh process is left behind per bootstrap"
  pass=$((pass + 1))
else
  echo "  FAIL  $left orphaned ssh process(es) left behind"
  fail=$((fail + 1))
fi

echo
printf 'passed %d, failed %d\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
