# One image, two roles. The client and the host run the same binary — a remote
# host *is* a medulla — so building twice would only create somewhere for the
# two to drift.
#
# The base must match the machine the binary was built on, because compose mounts
# `target/debug` straight in rather than rebuilding: a dynamically linked medulla
# wants that host's glibc and OpenSSL, and an older base fails at the loader with
# a wall of `version not found`. Bump this when the dev box moves.
ARG BASE=ubuntu:26.04
FROM ${BASE}

RUN apt-get update && apt-get install -y --no-install-recommends \
      openssh-server openssh-client ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

# A throwaway key, generated at build time and shared by both containers. It
# never leaves this image and the image is never published: it exists so the
# suite can run unattended, and for no other reason.
RUN ssh-keygen -t ed25519 -N "" -f /root/.ssh/id_ed25519 -q \
    && cp /root/.ssh/id_ed25519.pub /root/.ssh/authorized_keys \
    && chmod 600 /root/.ssh/authorized_keys

# Host-key checking off, because the host key is minted fresh on every run and
# there is no prior trust to establish. This is a disposable test network, not a
# pattern to copy.
RUN printf 'StrictHostKeyChecking no\nUserKnownHostsFile /dev/null\nLogLevel ERROR\n' \
      >> /etc/ssh/ssh_config \
    && sed -i 's/^#*PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config

# The binary under test, built on the host and mounted in by compose.
#
# A symlink rather than a `PATH` entry, because `ssh host -- medulla …` runs a
# non-login, non-interactive shell: sshd sets its own PATH and sources no profile,
# so an `ENV PATH` here would be invisible to exactly the invocation that matters.
# `/usr/local/bin` is on sshd's default PATH. The target does not exist until
# compose mounts it, which is fine — a symlink resolves when it is used.
ENV PATH="/opt/medulla:${PATH}"
RUN ln -s /opt/medulla/medulla /usr/local/bin/medulla

# A stand-in coding agent, so the suite can prove a *remote* one is launched
# with the same environment a local one gets. Installed under the name medulla
# detects, since detection is a PATH probe.
COPY bin/fake-claude /usr/local/bin/claude
RUN chmod +x /usr/local/bin/claude

WORKDIR /work
