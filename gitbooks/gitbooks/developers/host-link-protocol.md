---
description: >-
  The normative wire specification for medulla-link/1, the transport between a
  Medulla orchestrator and its remote hosts.
---

# Host link protocol (`medulla-link/1`)

The transport between a Medulla orchestrator and its remote hosts. It replaces
the tiny.place mailbox: instead of Signal-encrypting a frame, pushing it to a
hosted relay and polling it back down, endpoints exchange UDP datagrams carrying
mosh-style state synchronisation through a backend that forwards bytes it cannot
read.

This document is normative. Three implementations code against it: the
orchestrator endpoint, the host endpoint (both in the `medulla-link` crate) and
the forwarder that relays between them. Where this document and an
implementation disagree, this document is right.

## 1. Model

```
  orchestrator ──┐                          ┌── host A
   (endpoint)    ├─►  backend forwarder  ◄──┤
                 │    (blind, UDP)          └── host B
                 └──────────────────────────────┘
                     opaque payload, cleartext header
```

Two layers, deliberately separated:

| Layer | Key | Who can read it | Purpose |
|---|---|---|---|
| Outer header | forwarder key | backend and endpoints | routing, roaming, replay defence |
| Inner payload | pair key | endpoints only | the actual messages |

The backend holds forwarder keys and never holds a pair key. It therefore knows
who talks to whom, when, and how much, and nothing else. The pair key is
generated on the orchestrator, displayed to the user, and typed in by hand on the
host (section 7). It never appears in a request body, a response, a log line or
the database.

### Roles and direction

Every endpoint is either the orchestrator or a host. The role is fixed at
enrollment and determines the direction bit (section 4.2). It is not a property
of a given datagram.

## 2. Identifiers

`node_id` is 16 random bytes, issued by the backend at enrollment. This is what
travels on the wire.

`node_name` is human-readable, unique within a team, shown in the TUI and used as
a `Bridge` address. It lives in the registry and never on the wire; endpoints
resolve name to id once, at enrollment.

`team` is the tenancy boundary. The forwarder refuses to move a datagram between
teams (section 5, rule 6).

## 3. Outer header

Fixed 66 bytes, big-endian, cleartext. These are the only bytes the forwarder
parses.

| Offset | Size | Field | Notes |
|---:|---:|---|---|
| 0 | 1 | `version` | `2`. Anything else is dropped |
| 1 | 1 | `flags` | bit 0 = heartbeat (no state change). Bits 1 to 7 reserved, MUST be 0 |
| 2 | 16 | `src_node_id` | |
| 18 | 16 | `dst_node_id` | |
| 34 | 8 | `seq` | section 3.1 |
| 42 | 8 | `epoch` | random value minted for each endpoint process (section 6.4) |
| 50 | 16 | `tag` | `HMAC-SHA256(forwarder_key, bytes[0..50])[0..16]` |
| 66 | ... | `payload` | opaque to the forwarder (section 4) |

### 3.1 `seq`

A single counter per sending node, incremented once per datagram sent, regardless
of destination. It starts at 1 and never repeats for the life of the node's key
material. Bit 63 is the direction bit (section 4.2) and is therefore constant for
a given node; the low 63 bits are the counter.

`seq` has two purposes at once, which is why it is in the cleartext header:

1. the forwarder's replay window and rebinding rule (section 5), and
2. the AEAD nonce for the payload (section 4.2).

A node MUST persist enough of its counter to guarantee it never rewinds across
restarts. `medulla-link` does this by persisting a reservation: it writes
`counter + 10_000` to disk and only rewrites after consuming the reservation, so
a crash costs at most one skipped block, never a reused nonce. A nonce MUST never
repeat under a shared AEAD key, because reuse breaks confidentiality of the
payload.

### 3.2 Size

A datagram MUST NOT exceed 1400 bytes total. This keeps it inside a typical
1500-byte path MTU with room for IPv6 and any tunnelling, so datagrams are never
fragmented. A fragmented UDP datagram is dropped whole if any fragment is lost,
which would defeat the loss-tolerance the design is built on.

The payload budget is therefore `1400 - 66 (header) - 16 (AEAD tag) - 4
(timestamps) = 1322` bytes per Instruction, diff included. Senders MUST fragment
at the state layer (by sending a smaller diff), never at the datagram layer.

## 4. Payload

`payload = ChaCha20-Poly1305(key = pair_key, nonce = section 4.2, aad =
header[0..50], plaintext = section 4.1)`.

Binding the AAD to the outer header means a recipient can verify the datagram was
addressed to it, by that sender, at that sequence. A forwarder that redirected a
datagram to a different node would produce an authentication failure rather than
a delivered message.

### 4.1 Plaintext

| Offset | Size | Field |
|---:|---:|---|
| 0 | 2 | `send_ts`: sender's clock in ms, mod 2^16 |
| 2 | 2 | `reply_ts`: the last `send_ts` received from the peer, or 0 |
| 4 | ... | `Instruction` (section 4.3) |

`send_ts` and `reply_ts` are mosh's RTT probe. The receiver echoes `send_ts` back
in its next `reply_ts`; the original sender computes RTT as `now - reply_ts`,
which feeds SRTT and RTTVAR (section 6.1). A `reply_ts` of 0 means there is no
sample yet, and must not be read as an RTT of zero.

### 4.2 Nonce

12 bytes: 4 zero bytes followed by `seq` as a 64-bit big-endian integer, the same
`seq` as the outer header, direction bit included.

```
DIRECTION_MASK = 1 << 63     set   = orchestrator → host
                             clear = host → orchestrator
```

The direction bit is what makes a single pair key safe for both directions: the
two endpoints draw from disjoint halves of the counter space, so neither can
collide with the other. This is mosh's construction and the reason for it.

### 4.3 `Instruction`

| Size | Field | Notes |
|---:|---|---|
| 1 | `channel` | section 4.4 |
| 8 | `old_num` | state this diff applies to |
| 8 | `new_num` | state it produces |
| 8 | `ack_num` | highest state of the peer's stream we have applied |
| 8 | `throwaway_num` | peer may discard its states below this |
| 4 | `diff_len` | |
| ... | `diff` | channel-specific (section 4.4) |

The receiver applies a diff only when `old_num` equals the number of the state it
currently holds. Otherwise it drops the Instruction and does nothing; the sender
learns the true state from the next `ack_num` and re-diffs from there. This is
the whole reliability mechanism. Loss, reordering and duplication need no special
handling, because applying the same Instruction twice is a no-op and applying a
stale one is refused.

`throwaway_num` lets each side bound its history. A state below the peer's
`throwaway_num` can never be diffed from again, so it is freed.

### 4.4 Channels

One link multiplexes independent state streams. Each channel keeps its own
`sent_states` history, numbering, and ack, so an Instruction on channel 0 says
nothing about channel 1.

| Id | Channel | Semantics | Diff |
|---:|---|---|---|
| 0 | `messages` | reliable, ordered, nothing dropped | append-only (section 4.5) |
| 1 | `screen` | latest-wins | changed rows |

The split exists because the two streams have opposite requirements. Task frames
must all arrive: channel 0 is an append-only queue and a peer that was away
receives everything it missed. A terminal wants only the latest state: after a
40-second outage the host should send the current screen rather than 40 seconds
of scrollback. Channel 1 diffs the grid, so catching up costs one diff regardless
of how long the link was down. `src/sdk/src/protocol/screen/` already models
exactly this (`build_frame`, `apply_frame`, `changed_rows`) and supplies the
channel-1 diff directly.

### 4.5 Channel 0 diff

```
count  u32
repeat count times:
  len  u32
  body len bytes
```

The state is the sequence of messages appended so far; its number is the count of
messages. A diff from `old_num` to `new_num` is exactly messages `old_num+1`
through `new_num`. `apply_diff` appends.

A sender MUST bound its outbound queue. On overflow the link surfaces a transport
error rather than growing without limit; `hub/socket/task_run.rs::is_retryable`
already treats that as retryable, so it rejoins the existing retry path.

## 5. Forwarder rules

The forwarder is a pure function of the header plus its binding table. In order,
dropping silently unless stated:

1. `len >= 66` and `version == 2`.
2. `src_node_id` resolves to a known, non-revoked node. (Unknown: drop.)
3. `tag` verifies against that node's forwarder key, compared in constant time.
4. Replay window. Keep `highest_seq` and a 64-bit bitmap of the 64 sequences
   below it. Drop if `seq <= highest_seq - 64`, or if `seq` is already marked.
5. Rebind only when `seq > highest_seq`. Then set the node's source address to
   the datagram's source address, advance `highest_seq`, and refresh
   `lastSeenAt`.

   This rule is load-bearing. A forwarder that rebinds on any datagram with a
   valid tag can have a node's binding stolen by an attacker replaying one
   captured datagram from their own address, turning roaming support into a
   traffic-hijacking primitive. Requiring a strictly-higher sequence means an
   attacker must produce a datagram the legitimate node has not sent yet, which
   the tag prevents.
6. `dst_node_id` resolves to a node in the same team as the source. Cross-team:
   drop and increment a counter; record it as a security event.
7. `dst` has a live binding. If not, drop; the sender's SSP will retransmit.
8. Forward `bytes` verbatim. The forwarder MUST NOT rewrite any field, including
   `seq`. Rewriting would break both the AAD binding and the receiver's nonce.

Bindings are in-memory with a TTL. Losing them (a forwarder restart) is
self-healing: the next authenticated datagram from each node rebinds it, bounded
by the heartbeat interval rather than by an operator.

### 5.1 Limits

Rate limiting has two layers, and which side of the HMAC each one falls on is a
matter of correctness.

The datagram size cap (section 3.2) is applied first, before anything else, and
costs nothing.

A per-source-address token bucket is charged before the HMAC. Everything up to
the HMAC (a header parse and a node lookup) is attacker-triggerable work, so it
must be metered by something available at that point. Set it well above the
per-node rate: several of a user's hosts can share one NAT address.

A per-node token bucket is charged after the HMAC verifies. Charging it earlier
would mean anyone who learns a node id could exhaust that node's quota with
forged packets, turning the rate limit into a denial of service against the very
node it protects.

A node id that does not resolve MUST be cached as unresolved, briefly. Otherwise
a flood of random node ids costs one registry lookup each, and an unauthenticated
flood converts directly into database load.

Both of those caches are keyed by attacker-chosen values, so both MUST be bounded
with eviction. An unbounded map lets an attacker exhaust memory, which is the
denial of service the buckets are there to prevent.

`online` is derived from recent traffic, not asserted by the endpoint.

## 6. Timing

### 6.1 Retransmission

`RTO = SRTT + 4·RTTVAR`, clamped to `[MIN_RTO, MAX_RTO]`, with SRTT and RTTVAR
maintained per peer from the `send_ts` and `reply_ts` samples in section 4.1.

| Constant | Value |
|---|---|
| `SEND_INTERVAL_MIN` | 20 ms |
| `SEND_INTERVAL_MAX` | 250 ms |
| `MIN_RTO` | 50 ms |
| `MAX_RTO` | 1000 ms |
| `HEARTBEAT` | 3000 ms |

An endpoint sends when it has an unacked state change and `RTO` has elapsed since
the last send, subject to `SEND_INTERVAL_MIN`; and unconditionally every
`HEARTBEAT` when idle.

The heartbeat is not optional. A NAT mapping for an idle UDP flow typically
expires in about 30 seconds, and both the mapping and the forwarder binding
depend on traffic. 3 seconds is mosh's interval and there is no reason to differ.

### 6.2 Liveness

Derived from the last datagram received from the peer, and exposed on
`LinkHandle::status()`:

| State | Condition |
|---|---|
| `Live` | heard within `3 × HEARTBEAT` (9 s) |
| `Degraded` | 9 s to 60 s |
| `Offline` | over 60 s |

Liveness is advisory. SSP keeps retransmitting through all three states, so an
`Offline` peer can still come back, and recovery requires no reconnect, handshake
or re-enrollment. Nothing above the link may treat `Offline` as terminal on its
own.

### 6.3 Timeouts above the link

`TaskRunner` owns `ACK_WINDOW` (12 s) and `IDLE_WINDOW` (240 s). Those exist
because the tiny.place mailbox could silently black-hole a frame. This protocol
has no such failure mode.

Both clocks MUST be paused while liveness is not `Live`. Otherwise a 30-second
network blip fails a task that the transport was in the middle of recovering,
which would forfeit the entire reason for adopting SSP. They resume, rather than
reset, when liveness returns to `Live`: `ACK_WINDOW` measures peer processing,
and an unreachable peer is not processing anything.

The gate is per peer, not per link. An orchestrator holds sessions with many
hosts, and section 6.2 liveness is a property of one peer's session. Gating on an
aggregate would let a single dead host pause every other host's clock, so a task
dispatched to a healthy worker would stop timing out because an unrelated laptop
went to sleep. Implementations MUST evaluate liveness for the peer the task is
dispatched to.

A correct implementation needs both of these tests, because either alone passes
for the wrong reason: an outage longer than `ACK_WINDOW` must not fail the task
(proving the clock pauses), and a hung peer on a `Live` link must still time out
(proving the clock was gated rather than deleted).

### 6.4 Endpoint restart

Each endpoint process mints a random 64-bit `epoch` carried in every header. A
peer that observes the epoch change rebases its outbound state onto the shared
state 0, preserves any unconsumed messages and latest screen state, and resets
its inbound state to 0 before applying the restarted peer's instruction. This
prevents the old state-n / state-0 mismatch from wedging the link while retaining
work that was still awaiting delivery. The epoch is part of wire version 2;
version-1 datagrams are rejected rather than ambiguously interpreted.

## 7. Enrollment

Two secrets, issued by different parties, deliberately never combined in one
channel.

### 7.1 Pair key

128 bits, generated on the orchestrator, per host. Encoded for a human to retype:

```
payload   = 128-bit key ‖ 12-bit checksum        (140 bits)
checksum  = SHA-256(key)[0..12 bits]
encoding  = Crockford base32, no padding          (28 chars)
display   = 7 groups of 4, hyphen-separated
            e.g.  K3M9-2QRT-8XVA-P0WN-5JHD-6BZC-YE1F
```

Crockford base32 is chosen for transcription: it excludes `I`, `L`, `O` and `U`,
and decoding folds the confusable characters, so `0`/`O` and `1`/`I`/`L` typos
resolve rather than fail. The checksum catches the rest at entry, where the error
is obvious, instead of surfacing later as an unexplained decrypt failure.

The host reads it from its TTY, prompted. It MUST NOT be accepted as a
command-line flag: argv is world-readable via `ps` and lands in shell history.

### 7.2 Enroll token and forwarder key

```
POST /medulla/v1/hosts/invite
  → { token, expiresAt, orchestratorNodeId }

POST /medulla/v1/hosts/enroll   { token, name }
  → { nodeId, nodeName, forwarderKey, forwarderEndpoint, orchestratorNodeId }
```

* `token` is one-shot, short-TTL, and stored hashed.
* `forwarderKey` is returned exactly once and stored hashed thereafter.
* No enrollment request or response carries a pair key, in either direction. The
  backend has no code path that could receive one, and there is a test asserting
  it (section 9).

There is no key recovery. Since the backend never holds the pair key, a lost key
means re-enrolling the host.

### 7.3 State file

`<home>/link/node.json`, mode `0600`, holding the node id, role, pair key,
forwarder key, forwarder endpoint and the persisted sequence reservation
(section 3.1). Created and loaded under the same file lock used by the existing
identity bootstrap.

## 8. Scope

Every datagram goes through the forwarder. The protocol has no peer discovery, no
NAT traversal and no direct path between endpoints, so it is a relay topology
rather than a mesh.

Confidentiality covers payloads only. The backend sees the full social graph and
traffic volumes, so the protocol offers no metadata privacy.

Prediction sits outside this protocol. Mosh's local echo, where typed characters
appear instantly and reconcile against server truth, belongs to a terminal layer
above SSP. It can be built on top of this protocol and requires no change to it.

## 9. Conformance tests

An implementation is conformant when it passes these. They are listed here rather
than in a test file because they are part of the contract.

### Transport

* `apply_diff` is idempotent: applying the same Instruction twice equals applying
  it once.
* An Instruction whose `old_num` does not match the held state is refused, and the
  held state is unchanged.
* Convergence under 30% loss, reordering and duplication.
* A 60-second total blackout mid-task completes without error and without
  re-enrolling.
* Channel 1 catch-up after a 40-second outage converges in one diff to the
  current grid, rather than replaying the outage.
* Outbound queue overflow surfaces a retryable transport error.

### Forwarder

* A datagram with a bad tag is dropped.
* A source-address change with `seq > highest_seq` rebinds and traffic resumes.
* A captured datagram replayed from a different source address does not rebind
  (section 5, rule 5).
* A datagram addressed across teams is dropped and counted.
* Given a captured datagram, the forwarder cannot recover the payload.
* A flood of forged datagrams naming a real node does not consume that node's
  quota, and the node keeps working throughout (section 5.1).
* Per-source metering engages under a flood, and one flooding source does not
  starve another source.
* A node id repeatedly presented and never resolved is looked up once, not once
  per datagram.

### Enrollment

* The `HostNode` schema has no pair-key field, asserted against the schema itself
  so a later field addition fails.
* No enroll or invite validator accepts or returns a pair key.
* A mistyped pair key is rejected by checksum at entry.
* `join` refuses a pair key supplied as an argv flag.

### Integration

* A transient outage longer than `ACK_WINDOW` does not fail the task.
* A genuinely hung peer on a `Live` link does still time out, proving section 6.3
  gates the clocks rather than disabling them.

## Read next

* [Architecture](architecture.md): where the link sits in the code.
* [Testing](testing.md): the coordination end-to-end suite that drives this protocol.
* [Glossary](glossary.md#host-link): the term in context.
