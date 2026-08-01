# Authentication

Medulla authenticates to the backend with a JWT. You can supply it directly with
an environment variable, or — the easy path — log in through the browser and let
the CLI store a verified credential for you.

## `medulla login`

```sh
medulla login                       # google by default; opens the browser
medulla login --provider github     # google | github | twitter | discord
medulla login --no-browser          # just print the URL to open yourself
medulla login --token <64-hex>      # headless: redeem a one-time login token
```

`login` runs an [RFC 8252](https://datatracker.ietf.org/doc/html/rfc8252)
loopback flow: it binds a local `127.0.0.1:<port>` listener, sends you to the
backend's OAuth page, and captures the JWT the backend redirects back with. It
then verifies the token via `/auth/me`, prints who you are, and hands the JWT to
the embedded OpenHuman core as its app session. The base URL comes from
`backend.baseUrl` in the [config](configuration.md) (`--config <path>` to point
at a different config).

The core is the only place a session lives. Medulla used to keep its own
`credentials.json` beside the core's store, which meant two answers to "am I
signed in?" — `medulla login` could report success while the core, whose session
actually drives the runtime, stayed signed out and the TUI kept opening its login
screen. `login` and `logout` delete any such file left by an older install, since
nothing reads it any more and it holds a bearer token no logout could invalidate.

On the next `medulla` run the TUI finds the session and starts straight into the
app. `medulla logout` ends it — the session only: the account stays selected, so
signing back in returns to the same home and the same deployment. See
[Medulla home](configuration.md#medulla-home). Precedence for the backend token
stays: inline `backend.token` > `backend.tokenEnv` > the core's session.

## Token via environment

Skip login entirely and pass a JWT directly:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

## Logging in from the TUI

When you start `medulla` and the embedded core has no app session, the TUI opens
a login screen before the main app. There is no offline fallback: the mock
runtime is reached only by asking for it with `--mock`.

* **Enter / `o`** — start the browser loopback flow. The screen shows the login
  URL and waits for the callback on `127.0.0.1:<port>`; **Esc** cancels.
* **←/→** or **`p`** — cycle the provider (google / github / twitter / discord).
* **`t`** — paste a JWT or a 64-hex one-time login token (64 lowercase hex is
  redeemed via `/auth/login-token/consume`, anything else is treated as a JWT).
  **Enter** submits, **Esc** cancels.
* **`q`** / **Ctrl-C** — quit without starting the app.

On a token from either path the TUI verifies it via `/auth/me`, flashes who you
are, and hands it to the core, which validates it once more before storing it.
The app then starts on the embedded core — no restart. A core that cannot reach
Medulla at all (no backend URL, or the surface compiled out) stops with that
error rather than opening a login screen that could not fix it.

## Security model

The loopback listener hardens the callback against a hostile page sharing the
same `127.0.0.1` origin:

* A random 32-hex **state nonce** is appended to the `redirectUri` before it
  reaches the backend, and the listener rejects any `/auth` callback whose `state`
  is missing or mismatched (HTTP 400) while continuing to wait.
* It **drops non-loopback peers**, replies 405 to non-GET and 404 to non-`/auth`
  requests, and bounds each connection with a 5s read timeout and an 8 KiB
  buffer.

The session is written by the core, into its own credential store (the OS
keychain where one is available). Never commit tokens or `.env`; prefer
`MEDULLA_TOKEN` and documented environment variables over inline credentials in
committed config.
