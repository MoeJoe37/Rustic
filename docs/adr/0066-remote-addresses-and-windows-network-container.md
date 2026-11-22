# ADR 0066 — Remote addresses through the system resolver; the Windows network container

Date: 2026-11-22
Status: Accepted (Linux verified locally; the Windows container and native resolver behaviour wait for the next native CI run)

## Context

Until now the transport reached only origins the user routed to a
loopback address (`--allow-origin ORIGIN=ADDRESS`, ADR 0051, TD-157).
That kept hostile pages away from the Internet but also kept Rustic from
loading anything but fixtures, and it blocked confining the Windows
network process: an AppContainer cannot connect to loopback without a
machine-wide administrator exemption (TD-176).

Remote browsing needs name resolution and a rule for what a resolved name
may reach. The classic attack is DNS rebinding: a page's own host name
resolves to `127.0.0.1` or a LAN address and the browser connects to the
user's machine or network on the page's behalf.

## Decision

1. **The system resolver, in the broker.** `NetworkPolicy::address`
   resolves the host of an origin the user allowed with
   `std::net::ToSocketAddrs` (the platform's `getaddrinfo`). No resolver
   crate is added. The broker resolves; the network process never does,
   and receives only the address the broker chose (as for loopback
   routes, ADR 0061). Tests inject a fixed table
   (`Transport::with_resolver`).
2. **Grants.** `--allow-origin ORIGIN=ADDRESS` keeps routing one origin to
   a loopback fixture. `--allow-origin ORIGIN` lets one origin resolve.
   `--allow-remote` lets every `http`/`https` origin resolve. Fixed
   routes win for their origin.
3. **Public addresses only.** A resolved name (or an IP-literal host)
   connects only to the first public unicast address: never loopback,
   private (RFC 1918), shared (CGNAT), link-local, unique-local,
   documentation, benchmarking, multicast, reserved or unspecified
   addresses, looking through IPv4-mapped, NAT64 and 6to4 forms
   (`rustic_net::is_public`). Otherwise the request fails with
   `TransportError::NotPublic`, before any socket opens. Reaching a LAN
   host needs an explicit grant (a later loopback-style design for
   private networks, not part of M7).
4. **Defence in depth in the network process.** It re-checks every
   address it is told to connect to: loopback (a fixture grant) or public
   (`NetworkPolicy::allow_public`); anything else is refused.
5. **Windows network container.** When no loopback fixture origin is
   granted, the network process starts in its own less privileged
   AppContainer, `Rustic.Network` (`rustic_sandbox::Container::Network`),
   with the capabilities `internetClient`, `lpacCryptoServices` and
   `registryRead` (outbound Internet and certificate verification) and
   nothing else, in a kill-on-close job of one process with child
   processes restricted, exactly like the renderer launcher (ADR 0060).
   With a loopback fixture grant the process starts unconfined, because
   no AppContainer can reach loopback; that case serves only the user's
   own local servers.

## Alternatives considered

- **A resolver crate (hickory) or DNS-over-HTTPS.** More control
  (timeouts, caching, no `getaddrinfo` blocking) but a large reviewed
  dependency; the system resolver also honours the user's configuration
  (hosts file, VPN, enterprise DNS). Revisit for M8 performance work.
- **Resolving in the network process.** Keeps `getaddrinfo` parsing out of
  the broker, but then the broker could not enforce the public-address
  rule itself, and the Windows container would need more capabilities.
- **Allowing private addresses for resolved names.** Matches older
  browsers but is exactly the rebinding hole; Private Network Access
  style permission prompts can come later through the trusted strip
  (TD-173).

## Consequences

- Rustic can load remote pages for the first time; every origin still
  needs a grant, or `--allow-remote`.
- `getaddrinfo` blocks the calling thread without its own timeout; the
  request's total timeout does not cover resolution yet.
- On Windows the network process is confined for remote browsing. A run
  that mixes loopback fixture grants with remote origins starts it
  unconfined.

## Validation

- `rustic-net` policy tests: resolved origins reach only public
  addresses, the special-purpose address table, the network process's
  re-check.
- `resolved_origins_never_reach_local_addresses` (CLI, split and single
  process): `localhost` and `127.0.0.1` granted without an address are
  refused and nothing is sent.
- `the_windows_network_process_is_confined_for_remote_origins`: escape
  probes (read a file, list a directory, start `cmd.exe`, connect to
  loopback) are allowed unconfined and denied in the container, on the
  native Windows x64 and ARM64 runners.

## Revisit when

The M8 shell needs LAN access (printers, routers) through a prompt, DNS
must be cached or time-limited, or the container needs more capabilities
on a Windows release.
