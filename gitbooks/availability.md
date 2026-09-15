# Availability

Any registered account can use Medulla for its first 30 days without paying. After that it needs a Basic or Pro plan. The check runs when the terminal starts: `medulla login` signs you in, the binary asks the backend who you are, and an account past its 30 days on no paid plan sees a screen that says so, with a way to check again once you have subscribed and a way to sign out and use a different account.

The rule lives in the binary and only decides what the terminal shows. What an account may spend is metered by the backend.

The client itself is open source under GPL-3.0-only at [`tinyhumansai/medulla-src`](https://github.com/tinyhumansai/medulla-src), and `medulla --mock` runs the whole terminal offline against a scripted runtime with no account at all. Signing in is what a live account adds: the hosted sign-in, usage meters, and the feedback board.

Medulla is still early. If you run it on a workload we have not seen, tell us about it.
