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
then verifies the token via `/auth/me`, prints who you are, and saves credentials
to `<home>/credentials.json` (mode `0600` on unix) — e.g.
`~/.medulla/credentials.json`. For backward compatibility, a missing home file
falls back to reading the retired `<config-dir>/medulla/credentials.json`
location (nothing is moved). The base URL comes from `backend.baseUrl` in the
[config](configuration.md) (`--config <path>` to point at a different config).

On the next `medulla` run the TUI uses those stored credentials automatically,
provided their `baseUrl` matches the configured backend. `medulla logout` clears
the file. Precedence for the backend token stays: inline `backend.token` >
`backend.tokenEnv` > stored credentials.

## Token via environment

Skip login entirely and pass a JWT directly:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

## Logging in from the TUI

When you start `medulla` without `--core` and no token resolves — or the
stored/env token is expired or rejected (the `me()` preflight fails with an auth
error) — the TUI opens a login screen before the main app instead of silently
dropping to the mock:

* **Enter / `o`** — start the browser loopback flow. The screen shows the login
  URL and waits for the callback on `127.0.0.1:<port>`; **Esc** cancels.
* **←/→** or **`p`** — cycle the provider (google / github / twitter / discord).
* **`t`** — paste a JWT or a 64-hex one-time login token (64 lowercase hex is
  redeemed via `/auth/login-token/consume`, anything else is treated as a JWT).
  **Enter** submits, **Esc** cancels.
* **`m`** — continue offline with the [mock runtime](configuration.md#mock-zero-setup).
  **`q`** / **Ctrl-C** — quit.

On a token from either path the TUI verifies it via `/auth/me`, flashes who you
are, saves the credentials (a save failure is a non-fatal notice), and proceeds
into the app with a backend runtime. Explicit `--core` runs are never redirected
to this screen.

## Security model

The loopback listener hardens the callback against a hostile page sharing the
same `127.0.0.1` origin:

* A random 32-hex **state nonce** is appended to the `redirectUri` before it
  reaches the backend, and the listener rejects any `/auth` callback whose `state`
  is missing or mismatched (HTTP 400) while continuing to wait.
* It **drops non-loopback peers**, replies 405 to non-GET and 404 to non-`/auth`
  requests, and bounds each connection with a 5s read timeout and an 8 KiB
  buffer.

Credentials are written mode `0600` on unix. Never commit tokens, `.env`, or
`credentials.json`; prefer `MEDULLA_TOKEN` and documented environment variables
over inline credentials in committed config.
