---
status: diagnosed
trigger: "cloudflared tunnel returns HTTP 530 (error code 1033) when accessing OAuth callback via public hostname"
created: 2026-04-05T00:00:00Z
updated: 2026-04-05T00:00:00Z
---

## Current Focus

hypothesis: CONFIRMED — DNS CNAME points to wrong tunnel UUID
test: `cloudflared tunnel route dns right right.example.com` outputs "already configured to route to tunnel aaaabbbb" (the `right` tunnel with NO connections)
expecting: n/a — confirmed
next_action: Return root cause diagnosis

## Symptoms

expected: Requests to https://right.example.com/oauth/right/callback should be proxied through cloudflared to the local unix socket and return "missing state parameter"
actual: HTTP 530 with error code 1033 ("Argo Tunnel error") from Cloudflare edge. The request never reaches cloudflared or the local socket.
errors: "error code: 1033" from curl. Cloudflare dashboard shows "Published application" under Zero Trust.
reproduction: curl -s https://right.example.com/oauth/right/callback returns 530/1033. Local socket works fine.
started: Since tunnel creation during Phase 39

## Eliminated

## Evidence

- timestamp: 2026-04-05T00:00:00Z
  checked: tunnel connectivity
  found: Tunnel running with 4 active edge connections, ingress validates OK, rule matches correctly
  implication: Tunnel infrastructure is working — problem is between Cloudflare edge and tunnel

- timestamp: 2026-04-05T00:00:00Z
  checked: local socket
  found: curl --unix-socket returns "missing state parameter" — local service works
  implication: Backend is fine, problem is in Cloudflare routing layer

- timestamp: 2026-04-05T00:00:00Z
  checked: DNS
  found: CNAME already exists for right.example.com pointing to tunnel
  implication: DNS is configured

- timestamp: 2026-04-05T00:00:00Z
  checked: dashboard
  found: Route shown as "Published application" in Zero Trust
  implication: Possible Cloudflare Access policy intercepting traffic

- timestamp: 2026-04-05T19:31:00Z
  checked: tunnel list
  found: TWO tunnels exist — `right` (aaaabbbb, created 2026-04-04, NO connections) and `rightclaw` (bbbbcccc, created 2026-04-05, 4 active connections)
  implication: DNS CNAME could be pointing to the wrong tunnel

- timestamp: 2026-04-05T19:32:00Z
  checked: DNS CNAME target
  found: `cloudflared tunnel route dns right right.example.com` outputs "right.example.com is already configured to route to your tunnel tunnelID=aaaabbbb-0000-1111-2222-ccccddddeeee"
  implication: CNAME points to tunnel `right` (aaaabbbb) which has ZERO active connections

- timestamp: 2026-04-05T19:32:00Z
  checked: rightclaw global config
  found: config.yaml has tunnel_uuid "bbbbcccc-1111-2222-3333-ddddeeeeffff" (the `rightclaw` tunnel)
  implication: Running cloudflared uses `rightclaw` tunnel but DNS routes to `right` tunnel — UUID mismatch

- timestamp: 2026-04-05T19:32:00Z
  checked: route_dns behavior in cmd_init
  found: route_dns is non-fatal (warns on failure, continues). When cmd_init was re-run creating the `rightclaw` tunnel, route_dns failed silently because the CNAME already existed pointing to the old `right` tunnel
  implication: Root cause confirmed — stale DNS CNAME from previous tunnel creation was never updated

## Resolution

root_cause: DNS CNAME for right.example.com points to stale tunnel `right` (aaaabbbb) which has zero active connections, while the running cloudflared process serves tunnel `rightclaw` (bbbbcccc). Cloudflare edge resolves the hostname to the dead tunnel and returns 1033. The mismatch occurred because cmd_init was run twice — first creating `right`, then `rightclaw` — and route_dns is non-fatal, so the second run silently failed to update the CNAME (error 1003: record already exists).
fix: Delete the stale DNS CNAME record (via Cloudflare dashboard or API) and re-run `cloudflared tunnel route dns bbbbcccc-1111-2222-3333-ddddeeeeffff right.example.com` to point the CNAME to the active `rightclaw` tunnel. Optionally delete the unused `right` tunnel. Code-level fix: route_dns should detect when a CNAME exists but points to a DIFFERENT tunnel UUID and warn loudly (or offer to update it).
verification:
files_changed: []
