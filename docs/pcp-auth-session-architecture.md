## Project Control Plane Auth and Session Architecture

This document describes how authentication, session state, and Project Control Plane (PCP)
clients interact in datum-connect. It explains the watch relationships, what triggers what,
and where refresh/validation happens.

### Goals

- Centralize auth refresh logic in the auth subsystem.
- Keep selected org/project as persistent session state.
- Ensure PCP clients are always rebuilt when tokens change.
- Allow UI/CLI to use a simple, stable interface.

### Key Components

- `AuthClient` (`lib/src/datum_cloud/auth.rs`)
  - Owns OAuth tokens and refresh logic.
  - Emits watch notifications when auth state changes.
- `DatumCloudClient` (`lib/src/datum_cloud.rs`)
  - Owns selected context state (persisted).
  - Maintains in-memory org/project cache.
  - Builds PCP clients with current tokens.
  - Subscribes to auth updates to refresh org/project cache and validate context.
- `ProjectControlPlaneClient` (`lib/src/project_control_plane.rs`)
  - Holds a `kube::Client` for a specific project control plane.
  - Rebuilds its internal `kube::Client` when tokens change.

### State Ownership

- **Selected context** is persisted to `selected_context.yml` and owned by
  `DatumCloudClient` (not the node).
- **Org/project list** is cached in-memory by `DatumCloudClient` and refreshed
  on auth updates or explicit fetches.
- **Tokens** are owned by `AuthClient`.

### Watch Relationships

1) `AuthClient` emits updates
   - `login_state_watch()` emits `LoginState`.
   - `auth_update_watch()` emits a monotonically increasing counter on any auth change.

2) `DatumCloudClient` subscribes to `AuthClient`
   - On any auth update, it fetches org/project data and validates the selected context.
   - If the selected context is invalid, it clears it.
   - If valid, it re-emits the selected context for downstream watchers.

3) `ProjectControlPlaneClient` subscribes to `DatumCloudClient`
   - It watches login state and auth updates (re-exposed by `DatumCloudClient`).
   - On updates, it rebuilds the internal `kube::Client` if the token changed.

### Trigger Flows

#### Auth refresh scheduling

- `AuthClient` runs a refresh loop based on token expiry.
- When refresh occurs, `AuthClient` emits `auth_update_watch()` events.

#### Selected context

- UI/CLI sets selected context via `DatumCloudClient::set_selected_context(...)`.
- The selection is persisted to disk and emitted via `selected_context_watch()`.

#### Org/project cache

- `DatumCloudClient::orgs_and_projects()` refreshes the in-memory cache.
- On auth updates, `DatumCloudClient` refreshes the cache and validates the selection.

#### PCP client rebuild

- `ProjectControlPlaneClient` rebuilds when it receives an auth update.
- If the token did not change, the client is unchanged.


### Why this is clean

- **Single authority for auth**: refresh logic lives in `AuthClient`.
- **Separation of concerns**: `DatumCloudClient` owns session state and cloud data;
  PCP clients focus on Kubernetes access only.
- **Low coupling**: consumers subscribe to watch channels rather than hard dependencies.

### Future Enhancement

Add a light preflight hook for PCP clients to ensure token validity before each
Kubernetes API call. This would:

- Optionally call `AuthClient::load_refreshed()` at call time.
- Rebuild the `kube::Client` synchronously if the token changed.
- Avoid reliance on the background auth refresh loop for correctness.

This can be implemented as a thin wrapper around the `kube::Api` accessors, or
as an async `pcp.api_refreshed()` accessor that returns a fresh `Api<T>`.
