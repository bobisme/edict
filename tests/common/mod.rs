//! Shared helpers for the hermetic integration tests.
//!
//! Every test that runs `edict`/`rite` for real sandboxes `HOME`,
//! `RITE_DATA_DIR`, and the `XDG_*` vars against a private tempdir. It must
//! also sandbox `VESSEL_SOCKET`: vessel ignores `XDG_RUNTIME_DIR` for its
//! socket (it hardcodes `/run/user/$UID/vessel.sock` unless `--socket`/
//! `VESSEL_SOCKET` says otherwise) and its auto-started server re-execs into
//! the fixed systemd unit `vessel-server.scope` — the same unit the
//! machine's real vessel server already owns. Without `VESSEL_SOCKET`, a
//! rite hook that fires `vessel spawn` during a test would reach the REAL
//! vessel server instead of a sandboxed one (see bn-61kf). None of these
//! tests currently make a hook fire, but every test that registers a live
//! hook in a sandboxed `RITE_DATA_DIR` sandboxes `VESSEL_SOCKET` too, so a
//! hook that starts firing later stays contained.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Agent-identity env vars that route a `rite`/`vessel` call as a live
/// agent. Tests that must look like a passive human/CI invocation (not an
/// agent) remove these explicitly.
pub const AGENT_IDENTITY_VARS: [&str; 3] = ["AGENT", "RITE_AGENT", "BOTBUS_AGENT"];

/// Where a sandboxed vessel server would listen, if a rite hook ever
/// actually fired `vessel spawn` during a test. Kept short and inside
/// `home` (well under the Unix socket `sun_path` limit) so a stray hook
/// talks to nothing real.
pub fn vessel_socket_path(home: &Path) -> PathBuf {
    home.join("vessel.sock")
}
