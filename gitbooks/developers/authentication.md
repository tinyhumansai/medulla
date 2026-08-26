# Authentication

Medulla authenticates to the backend with a JWT. You can supply one directly
through config or an environment variable, or sign in and let Medulla store a
verified session itself.

Two sign-in flows are available, chosen by where your browser is:

| | Use when | How it ends |
|---|---|---|
| Browser (loopback) | The terminal and a browser are on the same machine | The browser is redirected back to a local listener carrying the JWT |
| Code | SSH sessions, containers, anything with no local browser | You open a URL on any device and paste the code it shows back into the terminal |

The browser flow cannot work over SSH. The backend redirects your browser to
`http://127.0.0.1:<port>`, which is the loopback interface of the machine running
the *browser*, not the remote host running Medulla, so the listener there never
sees the callback. Use the code flow.

## Where the credential comes from

Three sources are consulted, in this order, and the first that yields a token
wins:

1. `backend.token` — a JWT written inline in the [config](configuration.md).
2. `backend.tokenEnv` — the name of an environment variable to read, default
   `MEDULLA_TOKEN`. An empty value is ignored rather than treated as a token.
3. The stored session, `session.json` in the account's
   [Medulla home](configuration.md#medulla-home).

Every backend-facing surface resolves the token through that one chain — the
TUI's readiness check, `medulla hub`, and `medulla login`/`logout` alike — so
there is no path where one part of Medulla considers you signed in and another
does not.

The first two are *external credential sources*: the operator stated them
explicitly, so they outrank anything Medulla stores for itself. That has two
visible consequences, both deliberate:

* `medulla login` refuses to save a session underneath one. It signs you in,
  reports who you are, and then fails with `signed in, but <source> outranks the
  stored session — this shell would keep using that credential, so the login was
  not saved`. Storing a session that nothing would ever read is worse than
  refusing.
* `medulla logout` clears the store, then checks the chain again. If an external
  source still yields a token it exits non-zero: `the stored session was cleared,
  but this shell is still authenticated from <source> — remove it to finish
  signing out`. Logout means no source authenticates you, not "one of three was
  emptied".

### The stored session is scoped to its issuer

`session.json` records the `baseUrl` the token was issued by alongside the token
itself, and the runtime hands the stored bearer only to a `backend.baseUrl` whose
origin — scheme, host and port, normalized — matches it. Point a config at a
different deployment and the stored session is simply not offered to it; sign in
again against that deployment instead. `backend.token` and `backend.tokenEnv` are
not filtered this way, because those are the operator's own explicit choice.

## `medulla login`

```sh
medulla login                       # google by default; opens the browser
medulla login --provider github     # google | github | twitter | discord
medulla login --no-browser          # just print the URL to open yourself
medulla login --code                # paste-a-code flow (works over SSH)
medulla login --token <64-hex>      # non-interactive: redeem a code you already have
```

By default `login` runs an [RFC 8252](https://datatracker.ietf.org/doc/html/rfc8252)
loopback flow: it binds a local `127.0.0.1:<port>` listener, sends you to the
backend's OAuth page, and captures the JWT the backend redirects back with.

With `--code` it binds nothing. It prints
`<baseUrl>/auth/<provider>/login?redirect=cli` and waits on stdin. Open that URL
anywhere — another terminal's browser, your laptop, your phone — sign in, and the
page shows a one-time code. Paste it back and the CLI exchanges it for a JWT via
`POST /auth/login-token/consume`. The code is single-use and expires after 15
minutes; the long-lived session is only ever issued to whoever redeems it, so
what crosses your clipboard is not a bearer token.

Either way, `login` then:

1. Verifies the token with `GET /auth/me` and prints who you are.
2. Refuses to continue if an external credential source outranks the store.
3. Adopts the account: sanitizes the account id the backend returned, and seeds
   that account's `config.toml` with the `backend.baseUrl` it signed in against
   (an existing, different value is reported rather than overwritten).
4. Verifies once more, writes `session.json`, and only then publishes the account
   marker.
5. Re-checks that the home it just wrote to is the home this process resolves,
   and fails loudly with the reason if not.
6. Sweeps up any legacy `credentials.json` left by an older install.

The base URL comes from `backend.baseUrl` in the [config](configuration.md); pass
`--config <path>` to point at a different one.

### What is stored, and where

| Path | Contents |
|---|---|
| `<home>/session.json` | `{ "token", "userId", "baseUrl" }` — the session itself |
| `<root>/active_user.toml` | `user_id = "..."` — which account directory this install reads |

`<root>` is `~/.medulla` (or `MEDULLA_HOME`, or `./.medulla` under
`MEDULLA_DEV`), and `<home>` is `<root>/<account id>` — `<root>/local` before
you have ever signed in. There is no OS keychain involved: `session.json` is
written to a temporary file created `0600`, fsynced, and renamed over the
destination, so a reader never sees a half-written credential and the file is
never world-readable even briefly.

The account marker is written by the credential store, after the session file
lands. Nothing can leave you with a marker pointing at an account whose session
failed to write.

### Account ids and `MEDULLA_USER`

Every account's directory is named by the id the backend reports, so that id must
be safe as a path segment: non-empty, not starting with `.`, and made only of
`A-Z a-z 0-9 _ -`. It is checked before it is ever joined into a path. A backend
response that names no account, or names an unusable one, is refused with an
explicit error rather than guessed at — there is nowhere correct to store the
session.

`MEDULLA_USER` pins the active account for one process without touching the
shared marker. Because it is a pin rather than a preference, Medulla refuses
anything that would make it a lie:

* Signing in as a different account than `MEDULLA_USER` names →
  `MEDULLA_USER pins account <a>, but this token belongs to <b>`. Nothing is
  stored.
* The same conflict inside a running TUI → nothing is stored and the shared
  account selection is left unchanged.

`MEDULLA_USER=local` is the escape hatch back to the pre-login home.

## `medulla logout`

`logout` clears `session.json` (running it twice is not an error), removes legacy
credential files under the account home, and then re-checks the precedence chain
as described above.

It deliberately leaves the account marker and the account's directory alone.
Logging out is not forgetting which account and which deployment this install
belongs to; signing back in returns you to the same home, config and logs. See
[Medulla home](configuration.md#medulla-home).

### Upgrading from a standalone credentials file

Older installs kept a `credentials.json` alongside the runtime's own store, which
gave two independent answers to "am I signed in?" — `medulla login` could report
success while the runtime stayed signed out and the TUI kept opening its login
screen. `login` and `logout` both delete any such file they find, since nothing
reads it now and it holds a bearer token no logout could invalidate.

## Token via environment

Skip login entirely and pass a JWT directly:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

This is `backend.tokenEnv` at its default name. Remember it outranks the stored
session for as long as it is exported: `medulla logout` will tell you so rather
than silently leaving you authenticated.

## Logging in from the TUI

When you start `medulla` with no usable credential, the TUI opens a login screen
before the main app. There is no offline fallback: the mock runtime is reached
only by asking for it with `--mock`.

Everything is one list, navigated with `↑↓` and `Enter`; nothing is bound to a
bare letter, so no stray keystroke can start a flow. `Ctrl-C` quits from
anywhere.

```
▸ Sign in with a browser
  Sign in with a code (SSH / no browser)
  Paste an API key
  ──────────────
  Read the docs
  Star us on GitHub
  Quit
```

Pick the method first, then the provider (google / github / twitter / discord):

* **Sign in with a browser** opens your browser and waits for the callback on
  `127.0.0.1:<port>`. `Esc` goes back.
* **Sign in with a code** shows the URL to open on any device and a field for the
  code that page produces. `Ctrl-O` tries to open the URL here anyway (best
  effort, since the point of this flow is that there may be nothing to open),
  `Enter` submits, `Esc` goes back.
* **Paste an API key** takes a JWT or key you already hold.

A submitted value is classified by shape: 64 lowercase hex is redeemed via
`/auth/login-token/consume`, anything else is treated as a ready-made JWT. A
rejected value leaves you on the same screen, with the verification URL still up
so you can fetch a fresh code.

On a token from any path the TUI verifies it via `/auth/me`, flashes who you are,
stores it, and continues into the app with no restart.

Signing in again from inside a running app is decided by which account the new
token belongs to:

* **The same account** — the session is stored and you carry on.
* **A different account** — the marker moves and that account's config is seeded,
  but no session is stored for it: `Signed in as a different account. Restart
  medulla to finish signing in as them.` A running process has the previous
  account's home, logs and workers open; adopting a new identity underneath that
  is not something a restart-free path can honestly promise.
* **Refused** — the backend named no account, named an unusable one, or
  `MEDULLA_USER` pins this process elsewhere.

## Security model

### The code flow

The code is a one-time, 15-minute `type: login` session token bound to the
account that just authenticated. It is not a JWT. It is redeemed exactly once
(`/auth/login-token/consume` deletes it atomically), and the long-lived app
session is issued to whoever redeems it. A code read off a shared screen or left
in a clipboard therefore buys an attacker one race against you, within 15
minutes, rather than a standing credential, and a code that has already been
pasted is inert.

Because there is no loopback listener, the state-nonce protections below do not
apply and are not needed; no local socket is involved in this flow at all.

### The loopback flow

The loopback listener hardens the callback against a hostile page sharing the
same `127.0.0.1` origin:

* A random 32-hex state nonce is appended to the `redirectUri` before it reaches
  the backend, and the listener rejects any `/auth` callback whose `state` is
  missing or mismatched (HTTP 400) while continuing to wait.
* It drops non-loopback peers, replies 405 to non-GET and 404 to non-`/auth`
  requests, and bounds each connection with a 5s read timeout and an 8 KiB
  buffer.

### Credentials and agent turns

An agent turn that runs a shell command never inherits Medulla's environment. The
child is spawned from a cleared environment with a scrubbed set explicitly
re-added: `MEDULLA_HOME` and `MEDULLA_USER` are always dropped, along with every
variable whose name contains `TOKEN`, `KEY`, `SECRET`, `PASSWORD`, `PASSWD`,
`CREDENTIAL`, `AUTH` or `SESSION`. See
[Environment Variables](environment-variables.md#what-an-agent-turn-cannot-see).

Never commit tokens or `.env` files; prefer `MEDULLA_TOKEN` and the documented
environment variables over inline credentials in committed config.
