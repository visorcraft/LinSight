<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# linsight-tunnel

mTLS bridge for the LinSight daemon's Unix socket. Lets a remote GUI /
CLI / Prometheus scraper talk to `linsightd` over a TCP+TLS link with
mutual authentication.

**Most users should prefer plain `ssh -L`** — same security guarantees
(host key + user shell auth), zero cert management. `linsight-tunnel`
exists for non-SSH topologies: kiosks, monitoring nodes without an SSH
account, or environments where SSH egress is restricted.

## Topology

```
┌──────────────────────┐                      ┌──────────────────────┐
│  remote machine      │                      │  desktop             │
│                      │                      │                      │
│  linsightd           │                      │  linsight (GUI/CLI)  │
│    │ unix sock       │                      │    │ unix sock       │
│  linsight-tunnel     │ ── TCP + mTLS ──>    │  linsight-tunnel     │
│    server :9443      │                      │    client            │
└──────────────────────┘                      └──────────────────────┘
```

The two `linsight-tunnel` processes are a transparent byte pipe. Bytes
written to the desktop's local Unix socket flow through TLS to the
remote daemon and back.

## Generate a dev cert pair

For a real deployment use whatever PKI you already trust (step-ca,
HashiCorp Vault, your org's internal CA). For a kick-the-tires test,
the `rcgen` Rust crate or any other dev-CA tool will do. Here's a
minimal recipe with `openssl`:

```bash
# Self-signed CA (one-time)
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 \
  -days 365 -nodes -subj "/CN=LinSight Test CA" \
  -keyout ca.key -out ca.pem

# Server cert (CN must match what the client uses for --server-name / SNI)
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj "/CN=remote.host.example" \
  -addext "subjectAltName=DNS:remote.host.example" \
  -keyout server.key -out server.csr
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
  -days 365 -extfile <(printf "subjectAltName=DNS:remote.host.example") \
  -out server.pem

# Client cert (SAN lets the server pin this client with --allow-san)
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj "/CN=desktop-1" \
  -addext "subjectAltName=DNS:desktop-1" \
  -keyout client.key -out client.csr
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
  -days 365 -extfile <(printf "subjectAltName=DNS:desktop-1") \
  -out client.pem
```

## Run

On the remote host (where `linsightd` is running):

```bash
linsight-tunnel server \
  --bind 0.0.0.0:9443 \
  --cert server.pem --key server.key --ca ca.pem \
  --socket /run/user/1000/linsight.sock \
  --allow-san desktop-1
```

On the desktop:

```bash
linsight-tunnel client \
  --listen $XDG_RUNTIME_DIR/linsight-remote.sock \
  --server remote.host.example:9443 \
  --cert client.pem --key client.key --ca ca.pem
```

Then connect with the GUI or CLI against the local socket:

```bash
linsight --socket $XDG_RUNTIME_DIR/linsight-remote.sock
# or
linsight-cli --socket $XDG_RUNTIME_DIR/linsight-remote.sock list
```

## Defaults and limits

- `--bind` defaults to `127.0.0.1:9443`. Pass `0.0.0.0:9443`
  explicitly to expose to the network — the localhost default avoids
  silently bridging the daemon to every interface on the host.
- `--max-connections` defaults to 64 on both sides. Excess
  connections are dropped *before* TLS auth so a connection burst
  cannot pre-auth-DoS the daemon.
- `--allow-cn` and `--allow-san` are server-side client-certificate
  filters. Repeat them to admit multiple client identities.
- Ctrl+C / SIGTERM triggers a graceful drain (10 s default budget for
  in-flight TLS sessions to send `close_notify`); past the budget,
  outstanding tasks are aborted so the process doesn't hang.
- Client mode removes its local listener socket on exit via a Drop
  guard; stale sockets from prior crashes are probed before being
  overwritten (no TOCTOU on the cleanup).

## Trust model

The server always requires a client certificate signed by the configured
CA bundle. If neither `--allow-cn` nor `--allow-san` is passed, any
client certificate chained to that CA is accepted, so **the CA trust
boundary == full daemon access**.

For tighter deployments, pass one or more allowlist filters on the
server:

- `--allow-cn VALUE` accepts a client certificate whose Subject
  CommonName matches `VALUE`.
- `--allow-san VALUE` accepts a client certificate with a DNS
  SubjectAltName matching `VALUE`.

Exact matches are case-insensitive. A leading wildcard such as
`*.example.com` matches one DNS label (`desktop.example.com`) but not
multiple labels (`a.b.example.com`) and not the bare suffix
(`example.com`). When any allowlist is configured, the client cert must
match at least one `--allow-cn` or `--allow-san` value after normal CA
chain validation succeeds.

## Tests

`cargo test -p linsight-tunnel` runs:

- `mtls_handshake_and_byte_round_trip` — generates a self-signed CA +
  server + client cert chain with `rcgen`, completes a mutual
  handshake, and verifies bytes flow in both directions.
- `mtls_rejects_untrusted_client_cert` — proves the server verifier
  actually rejects a cert signed by an out-of-bundle CA (catches the
  "verifier accidentally accepting anything" failure mode).
- Unit tests for CN/SAN extraction, allowlist matching, idle-copy
  behavior, and CLI help coverage for `--allow-cn` / `--allow-san`.
