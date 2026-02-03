# Heartbeat Agent

This document describes the heartbeat agent introduced for keeping connector status
and leases up to date in the Project Control Plane (PCP).

## Goals

- Ensure each project connector has correct `status.connectionDetails` based on the
  local listener (public key, addresses + ports, relay URL, DNS discovery mode).
- Renew connector leases using the controller-provisioned `leaseRef.name`.
- Avoid unnecessary PCP list calls and noisy polling when no connector exists.
- Allow UI/CLI to register or deregister heartbeat loops per project.

## Architecture

### Components

- `HeartbeatAgent` owns lifecycle and per-project loops.
- `HeartbeatDetailsProvider` abstracts how we derive connection details.
  The current implementation uses the local `ListenNode`.
- Per-project loop: resolves connector + lease, patches status, renews lease.

### Flow

1. **Startup**: UI creates `HeartbeatAgent` and calls `start()`.
2. **Login Watch**: on login or auth refresh, the agent calls `refresh_projects()`.
3. **Project Refresh**:
   - Fetch orgs/projects from `DatumCloudClient`.
   - Compare the project set with cached projects.
   - If unchanged, exit early.
   - For new projects, **probe once** for a connector by field selector
     `status.connectionDetails.publicKey.id=<endpoint_id>`.
   - Only start a per-project loop if a connector exists.
   - If a project disappears, stop its loop.
4. **Hooks**:
   - UI/CLI calls `register_project(project_id)` when a connector is created.
   - UI/CLI calls `deregister_project(project_id)` when the last connector is removed.
5. **Per-Project Loop**:
   - Cache connector name and `leaseRef.name` once discovered.
   - Patch `status.connectionDetails` when details change.
   - Renew the lease using `spec.renewTime` on a jittered interval based on
     `leaseDurationSeconds / 2`.
   - If `leaseRef` is missing, back off exponentially until it appears.

## Connection Details

The connection details are derived from the listener endpoint:

- `publicKey.id`: iroh endpoint id.
- `publicKey.addresses`: socket addresses (IP + port).
- `publicKey.homeRelay`: relay URL from the endpoint; if missing, the previous
  relay value from connector status is reused.
- `publicKey.discoveryMode`: DNS (default).

The agent patches only `status.connectionDetails` and avoids clobbering other
status fields such as `leaseRef`.

## Lease Renewal

Leases are renewed by patching:

```
spec.renewTime = MicroTime(now)
```

The loop interval is computed as:

- base = max(1s, leaseDurationSeconds / 2)
- jitter = random(0..=base/5)
- interval = base + jitter

If `leaseDurationSeconds` is missing, a default of 30 seconds is used.

## Caching and Efficiency

- Project list changes are detected by set comparison to avoid redundant probes.
- Connector lookups are only done:
  - once per project at startup/refresh, and
  - during per-project loops if cached data is incomplete.

## File Locations

- Implementation: `lib/src/heartbeat.rs`
- UI wiring: `ui/src/state.rs`
