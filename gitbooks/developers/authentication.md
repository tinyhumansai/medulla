# Authentication

Medulla authenticates to the backend with a JWT. You can supply it directly with
an environment variable, or sign in and let the CLI store a verified credential.

Two sign-in flows are available, chosen by where your browser is:

| | Use when | How it ends |
|---|---|---|
| Browser (loopback) | The terminal and a browser are on the same machine | The browser is redirected back to a local listener carrying the JWT |
| Code | SSH sessions, containers, anything with no local browser | You open a URL on any device and paste the code it shows back into the terminal |

The browser flow cannot work over SSH. The backend redirects your browser to
`http://127.0.0.1:<port>`, which is the loopback interface of the machine running
the *browser*, not the remote host running Medulla, so the listener there never
sees the callback. Use the code flow.

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
anywhere, on another terminal's browser, your laptop, or your phone, sign in, and
the page shows a one-time code. Paste it back and the CLI exchanges it for a JWT
via `POST /auth/login-token/consume`. The code is single-use and expires after 15
minutes; the long-lived session is only ever issued to whoever redeems it, so
what crosses your clipboard is not a bearer token.

Either way `login` then verifies the token via `/auth/me`, prints who you are,
and hands the JWT to the embedded OpenHuman core as its app session. The base URL
comes from `backend.baseUrl` in the [config](configuration.md) (`--config <path>`
to point at a different config).

The core is the only place a session lives. On the next `medulla` run the TUI
finds the session and starts straight into the app. `medulla logout` ends the
session and nothing else: the account stays selected, so signing back in returns
to the same home and the same deployment. See
[Medulla home](configuration.md#medulla-home). Precedence for the backend token
is inline `backend.token`, then `backend.tokenEnv`, then the core's session.

### Upgrading from a standalone credentials file

Older installs kept a `credentials.json` beside the core's store, which gave two
independent answers to "am I signed in?": `medulla login` could report success
while the core, whose session drives the runtime, stayed signed out and the TUI
kept opening its login screen. `login` and `logout` delete any such file left
behind, since nothing reads it now and it holds a bearer token no logout could
invalidate.

## Token via environment

Skip login entirely and pass a JWT directly:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

## Logging in from the TUI

When you start `medulla` and the embedded core has no app session, the TUI opens
a login screen before the main app. There is no offline fallback: the mock
runtime is reached only by asking for it with `--mock`.

Everything is one list, navigated with `↑↓` and `Enter`; nothing is bound to
a bare letter, so no stray keystroke can start a flow. `Ctrl-C` quits from
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

Pick the method first, then the provider (google / github / twitter):

* Sign in with a browser opens your browser and waits for the callback on
  `127.0.0.1:<port>`. `Esc` goes back.
* Sign in with a code shows the URL to open on any device and a field for
  the code that page produces. `Ctrl-O` tries to open the URL here anyway (best
  effort, since the point of this flow is that there may be nothing to open),
  `Enter` submits, `Esc` goes back.
* Paste an API key takes a JWT or key you already hold.

A submitted value is classified by shape: 64 lowercase hex is redeemed via
`/auth/login-token/consume`, anything else is treated as a ready-made JWT. A
rejected value leaves you on the same screen, with the verification URL still up
so you can fetch a fresh code.

On a token from any path the TUI verifies it via `/auth/me`, flashes who you
are, and hands it to the core, which validates it once more before storing it.
The app then starts on the embedded core, with no restart. A core that cannot
reach Medulla at all (no backend URL, or the surface compiled out) stops with
that error instead of opening a login screen that could not fix it.

## Security model

### The code flow

The code is a one-time, 15-minute `type: login` session token bound to the
account that just authenticated. It is not a JWT. It is redeemed exactly once
(`/auth/login-token/consume` deletes it atomically), and the long-lived app
session is issued to whoever redeems it. A code read off a shared screen or
left in a clipboard therefore buys an attacker one race against you, within 15
minutes, rather than a standing credential, and a code that has already been
pasted is inert.

Because there is no loopback listener, the state-nonce protections below do not
apply and are not needed; no local socket is involved in this flow at all.

### The loopback flow

The loopback listener hardens the callback against a hostile page sharing the
same `127.0.0.1` origin:

* A random 32-hex state nonce is appended to the `redirectUri` before it
  reaches the backend, and the listener rejects any `/auth` callback whose `state`
  is missing or mismatched (HTTP 400) while continuing to wait.
* It drops non-loopback peers, replies 405 to non-GET and 404 to non-`/auth`
  requests, and bounds each connection with a 5s read timeout and an 8 KiB
  buffer.

The session is written by the core, into its own credential store (the OS
keychain where one is available). Never commit tokens or `.env`; prefer
`MEDULLA_TOKEN` and documented environment variables over inline credentials in
committed config.
