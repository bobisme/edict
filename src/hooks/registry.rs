/// Claude Code hook event type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    PreCompact,
    PostToolUse,
    SessionEnd,
}

impl HookEvent {
    #[must_use] 
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::PreCompact => "PreCompact",
            Self::PostToolUse => "PostToolUse",
            Self::SessionEnd => "SessionEnd",
        }
    }
}

/// Hook registry entry — event-based hooks
#[derive(Debug, Clone)]
pub struct HookEntry {
    pub name: &'static str,
    pub events: &'static [HookEvent],
}

/// Global hook registry — hooks are named by when they fire, not what they do.
///
/// Each hook detects context at runtime (maw repo, edict project, $AGENT)
/// and outputs info accordingly, or exits silently.
pub struct HookRegistry;

impl HookRegistry {
    /// Get all registered hooks
    #[must_use] 
    pub fn all() -> Vec<HookEntry> {
        vec![
            HookEntry {
                name: "session-start",
                events: &[HookEvent::SessionStart, HookEvent::PreCompact],
            },
            HookEntry {
                name: "post-tool-call",
                events: &[HookEvent::PostToolUse],
            },
            HookEntry {
                name: "session-end",
                events: &[HookEvent::SessionEnd],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_hooks_registered() {
        let hooks = HookRegistry::all();
        assert_eq!(hooks.len(), 3);
        assert!(hooks.iter().any(|h| h.name == "session-start"));
        assert!(hooks.iter().any(|h| h.name == "post-tool-call"));
        assert!(hooks.iter().any(|h| h.name == "session-end"));
    }
}
