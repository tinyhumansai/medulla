---
description: >-
  The normative wire specification for medulla-link/1, the transport between a
  Medulla client and its remote hosts.
---

# Host link protocol (`medulla-link/1`)

The transport between the Medulla you are looking at and its remote hosts.
Endpoints exchange UDP datagrams carrying mosh-style state synchronisation,
straight from one machine to the other. There is no relay, no backend in the
path, and nothing to sign in to: the only thing the two ends share is a pair
key, minted on one of them and carried to the other once.

Two words in this specification are protocol role names, not product terms.
The **owner** is the client end: the machine you run `medulla` on, which opens
sessions elsewhere. The **host** is the machine serving them. The role is fixed
when the pair is minted and decides the direction bit (section 4.2).

Two bootstraps produce a pair, and both end in the same link:

* **SSH-bootstrapped.** A `[[remoteHosts]]` entry is reached over SSH, the
  client starts `medulla daemon --direct` there, the host mints the pair key and
  prints it back up the SSH channel, and SSH is not used again.
* **Paired.** The client mints a host key (section 7.2) and the operator supplies
  it to the paired-daemon invocation on the host, which then serves that one
  client on the port the key names, across the client coming and going.

This document is normative. Both endpoints live in the `medulla-link` crate and
code against it. Where this document and the implementation disagree, this
document is right.

## 1. Model

```
  owner ─────────────────────────────────────── host
  (endpoint)      UDP, one socket, one peer      (endpoint)
                  opaque payload, authenticated cleartext header
```

Two layers, deliberately separated:

| Layer | Key | Purpose |
|---|---|---|
| Outer header | path key, derived from the pair key | addressing, roaming, cheap rejection of off-path junk |
| Inner payload | AEAD key, derived from the pair key | the actual messages |

Both derived keys are held by the two endpoints and nobody else. The path key
is not a second secret: it is derived from the pair key under its own label
(section 5), exactly as the AEAD key is, and anything that could forge it could
already decrypt. Its job is to reject a datagram that is not from the peer
before any AEAD work is done.

The pair key never appears in a config file, a log line or a request body. It
lives in the state file (section 7.3) on each end and nowhere else.

### Roles and direction

Every endpoint is either the owner or a host. The role is fixed when the pair
is minted and determines the direction bit (section 4.2). It is not a property
of a given datagram.

## 2. Identifiers

`node_id` is 16 random bytes per endpoint, minted with the pair. This is what
travels on the wire; a datagram naming a node id other than the two in the pair
is dropped.

`node_name` is human-readable, shown in the TUI and used as a `Bridge` address.
It lives in local configuration (`[[remoteHosts]].name`, `[link].nodeName`) and
never on the wire.

## 3. Outer header

Fixed 66 bytes, big-endian, cleartext. These are the only bytes readable
on the path.

| Offset | Size | Field | Notes |
|---:|---:|---|---|
| 0 | 1 | `version` | `2`. Anything else is dropped |
| 1 | 1 | `flags` | bit 0 = heartbeat (no state change). Bits 1 to 7 reserved, MUST be 0 |
| 2 | 16 | `src_node_id` | |
| 18 | 16 | `dst_node_id` | |
| 34 | 8 | `seq` | section 3.1 |
| 42 | 8 | `epoch` | random value minted for each endpoint process (section 6.4) |
| 50 | 16 | `tag` | `HMAC-SHA256(path_key, bytes[0..50])[0..16]` (section 5.1) |
| 66 | ... | `payload` | opaque on the path (section 4) |

### 3.1 `seq`

A single counter per sending node, incremented once per datagram sent, regardless
of destination. It starts at 1 and never repeats for the life of the node's key
material. Bit 63 is the direction bit (section 4.2) and is therefore constant for
a given node; the low 63 bits are the counter.

`seq` has two purposes at once, which is why it is in the cleartext header:

1. the receiver's replay defence and roaming rule (section 5), and
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

`payload = ChaCha20-Poly1305(key = aead_key, nonce = section 4.2, aad =
header[0..50], plaintext = section 4.1)`.

```
aead_key = SHA-256("medulla-link/1 aead" ‖ pair_key)
```

`aead_key` is the 32-byte output of SHA-256. The pair key remains the 16-byte
pairing secret; it is never passed directly to ChaCha20-Poly1305.

Binding the AAD to the outer header means a recipient can verify the datagram was
addressed to it, by that sender, at that sequence. Anything on the path that redirected a
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
DIRECTION_MASK = 1 << 63     set   = owner → host
                             clear = host → owner
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
error rather than growing without limit; the hub's dispatch loop treats that as
retryable, so it rejoins the existing retry path.

## 5. Path rules

With no relay in the path, the endpoint itself does the three jobs a relay
would otherwise do: it authenticates the cleartext header, it decides which
source addresses to believe, and it follows a peer that moves. Each has one
rule.

### 5.1 Path key

```
path_key = SHA-256("medulla-link/1 direct-path" ‖ pair_key)
```

The label is different from the AEAD key's, so the two derivations can never
collide and the construction is not silently reusable for anything else. The
header `tag` (section 3) is `HMAC-SHA256(path_key, bytes[0..50])[0..16]` and is
compared in constant time. A datagram whose tag does not verify is dropped
before the payload is touched.

### 5.2 Accepting a source address

An endpoint accepts a datagram from **any** source address and defers the
decision to authentication. "A datagram from an address we have not seen
before" is precisely what roaming looks like, so filtering on address would
break the one property the design exists for.

### 5.3 Roaming

The peer's address starts where the bootstrap said and moves under exactly one
rule: adopt the source address of a datagram that both authenticates (the tag
verifies and the payload decrypts) **and** advances the highest sequence seen
from that peer.

The freshness half rejects any replay received after the original sequence has
been accepted. Before an endpoint receives the original datagram, an attacker
who can replay it from another address can temporarily move the peer address;
the next legitimate datagram with a higher sequence restores it. A deployment
that cannot tolerate that transient limitation MUST use a reachability path
that prevents captured datagrams from being replayed from a different source.

### 5.4 Limits

The datagram size cap (section 3.2) is applied first, before anything else, and
costs nothing. The tag check comes next; everything up to it (a header parse) is
attacker-triggerable work and is bounded by being trivially cheap. An
implementation MAY meter unauthenticated datagrams per source address; it MUST
NOT charge anything keyed on the peer's node id before the tag verifies, since
a forged flood naming the peer would otherwise exhaust that peer's quota.

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
expires in about 30 seconds, and the mapping depends on traffic. 3 seconds is mosh's interval and there is no reason to differ.

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
or re-pairing. Nothing above the link may treat `Offline` as terminal on its
own.

### 6.3 Timeouts above the link

`TaskRunner` owns `ACK_WINDOW` (12 s) and `IDLE_WINDOW` (240 s). Those exist
because an earlier store-and-forward relay could silently black-hole a frame.
This protocol has no such failure mode.

Both clocks MUST be paused while liveness is not `Live`. Otherwise a 30-second
network blip fails a task that the transport was in the middle of recovering,
which would forfeit the entire reason for adopting SSP. They resume, rather than
reset, when liveness returns to `Live`: `ACK_WINDOW` measures peer processing,
and an unreachable peer is not processing anything.

The gate is per peer, not per link. An owner holds sessions with many
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

## 7. Pairing

One secret, the pair key, minted on one endpoint and carried to the other once.
How it travels depends on the bootstrap, but it never travels through a third
party and never sits in argv on the typed path.

### 7.1 Pair key

128 bits, generated per host. Encoded for a human to retype:

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

On the SSH-bootstrapped path the host mints the key and prints it back inside
the SSH channel; the client stores it and dials. When a pair key is typed, it is
read from a TTY, prompted. It MUST NOT be accepted as a command-line flag: argv
is world-readable via `ps` and lands in shell history.

### 7.2 Host key

The paired bootstrap has no channel back to the client, so everything the host
needs to come up is bundled into one string the operator pastes once:

```
raw      = 0x01 ‖ owner node id (16) ‖ host node id (16) ‖ pair key (16) ‖ udp port (2, BE)
           ‖ checksum (2) = SHA-256(raw[0..51])[0..2]                       (53 bytes)
encoding = Crockford base32 of 424 bits                                     (85 chars)
display  = "HK1-" + groups of 4, hyphen-separated
```

The key is pasted, not typed — it lands on the clipboard when it is minted — so
its length is not the constraint the pair key's is. It keeps the pair key's
alphabet and folding so a copy that went through a chat window survives, and the
checksum turns a truncated paste into an error at entry rather than a link that
never comes up.

The pair key rides in argv here, which section 7.1 forbids for the typed key.
The trade is deliberate: a one-shot command that works on any box is the whole
point of pairing this way, but local processes that can inspect argv and readers
of shell history MUST be trusted for that start. On hosts that cannot meet that
assumption, use a supported protected input channel such as stdin or a
permission-restricted file descriptor. A re-issued key always carries a fresh host node id
and pair key; re-pairing must not be a way to recover the old one.

### 7.3 State file

`<home>/link/node.json`, mode `0600`, holding the node id, role, the peer's
node id and pair key, and the persisted sequence reservation (section 3.1).
Created and loaded under a file lock, and only ever one `Link` per state
directory: a second link on the same node would draw sequences from a second
counter under one AEAD key, which reuses nonces.

There is no key recovery. A lost state file means pairing the host again.

## 8. Scope

Each link is one socket talking to one peer. The protocol has no peer discovery
or NAT traversal. Roaming and the heartbeat preserve an already reachable path;
they do not create a NAT mapping. A host behind a NAT therefore needs an opened
or forwarded UDP port, or an existing compatible outbound mapping, before the
owner can reach it.

Confidentiality covers payloads; the cleartext header exposes the two node ids,
the sequence and the epoch to anyone on the path, so the protocol offers no
metadata privacy against a network observer.

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
* A 60-second total blackout mid-session completes without error and without
  re-pairing.
* Channel 1 catch-up after a 40-second outage converges in one diff to the
  current grid, rather than replaying the outage.
* Outbound queue overflow surfaces a retryable transport error.

### Path

* A datagram with a bad tag is dropped before any AEAD work.
* A source-address change carried by a datagram with `seq > highest_seq`
  moves the peer address and traffic resumes.
* A captured datagram replayed from a different source after its original has
  been accepted does not move the peer address; a replay before first receipt
  may move it temporarily and is corrected by the next fresh peer datagram
  (section 5.3).
* A datagram naming a node id outside the pair is dropped.
* Both ends derive the same path key from the pair key, and it differs from the
  AEAD key.

### Pairing

* A mistyped pair key is rejected by checksum at entry.
* A truncated host key is rejected by length and checksum at entry.
* The typed pair key is refused when supplied as an argv flag.
* A re-issued host key carries a fresh host node id and pair key.

### Integration

* A transient outage longer than `ACK_WINDOW` does not fail the session.
* A genuinely hung peer on a `Live` link does still time out, proving section 6.3
  gates the clocks rather than disabling them.

## Appendix A. Relayed route (test harness only)

The `medulla-link` crate keeps a second route, `LinkPath::Forwarder`, in which
both endpoints send every datagram to one fixed relay address and the relay
routes on the cleartext header, authenticating the `tag` under a per-node relay
key it shares with that node rather than under the path key. Nothing deployed
implements that relay and there is no way to obtain a relay key: the route
exists so the in-process coordination suite can put a blind relay between two
endpoints and assert what it can and cannot see (`mock_link_forwarder` in the
SDK examples). The payload layer, the sequence rules and the state
synchronisation are identical on both routes; only section 5 differs, and the
relayed variant of it is documented on the example itself.

## Read next

* [Architecture](architecture.md): where the link sits in the code.
* [Testing](testing.md): the coordination end-to-end suite that drives this protocol.
* [Glossary](glossary.md#host-link): the term in context.
