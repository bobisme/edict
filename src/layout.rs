//! Workspace layout detection.
//!
//! maw historically enforced a **bare** repo layout: the project root was a bare
//! git repo with no source files, the trunk working copy lived at `ws/default/`,
//! and every agent workspace lived at `ws/<name>/`. Running anything against the
//! trunk therefore required the `maw exec default -- <cmd>` prefix.
//!
//! Starting with maw `v1.0.0-pre.2`, the bare layout is no longer enforced. In the
//! new **root** layout the project root *is* the trunk working copy (a normal git
//! checkout — `src/`, `.bones/`, `.edict.toml`, `AGENTS.md` all live at the root),
//! and extra agent workspaces live under `.maw/workspaces/<name>/`. Trunk-context
//! commands (`bn`, `cargo`, `seal` on the trunk) run directly at the root with no
//! prefix.
//!
//! Edict generates docs and runtime guidance for downstream projects on *either*
//! layout, so it detects which one a project uses and renders layout-appropriate
//! paths and command prefixes. What stays identical across both layouts:
//! `maw exec <ws> -- <cmd>` for non-default workspaces, `maw ws create/merge/list`,
//! and `--into default` as the merge target (in the root layout, `default` still
//! resolves to the repo root).

use std::path::Path;

/// Which on-disk workspace layout a maw project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Legacy bare repo: trunk at `ws/default/`, workspaces at `ws/<name>/`.
    /// Trunk commands require `maw exec default -- <cmd>`.
    Bare,
    /// Root layout (maw >= v1.0.0-pre.2): trunk is the repo root, workspaces at
    /// `.maw/workspaces/<name>/`. Trunk commands run directly at the root.
    Root,
}

impl Layout {
    /// Detect the layout from a project root.
    ///
    /// Robust whether `project_root` is the bare repo root *or* the bare trunk
    /// worktree itself (`…/ws/default`) — the latter matters because `edict sync`
    /// recurses into `ws/default` via `maw exec default -- edict sync`, at which
    /// point the trunk looks like an ordinary checkout.
    ///
    /// - At the bare repo root, a `ws/default/` directory is the definitive marker.
    /// - Inside the bare trunk, the path ends in `ws/default` and the bare repo
    ///   root two levels up carries maw's `.manifold` metadata dir.
    /// - Everything else (a fresh checkout, or a root-layout repo with
    ///   `.maw/workspaces/`) is the root layout.
    #[must_use]
    pub fn detect(project_root: &Path) -> Self {
        // Standing at a maw bare repo root: ws/default is the trunk worktree.
        if project_root.join("ws/default").is_dir() {
            return Self::Bare;
        }
        // Standing inside the bare trunk (…/ws/default): the bare repo root is two
        // levels up and carries the maw `.manifold` metadata dir.
        if project_root.file_name().is_some_and(|n| n == "default")
            && project_root
                .parent()
                .is_some_and(|p| p.file_name().is_some_and(|n| n == "ws"))
            && let Some(bare_root) = project_root.parent().and_then(|p| p.parent())
            && bare_root.join(".manifold").is_dir()
        {
            return Self::Bare;
        }
        Self::Root
    }

    /// True for the new root layout.
    #[must_use]
    pub const fn is_root(self) -> bool {
        matches!(self, Self::Root)
    }

    /// Command prefix for running a command in the trunk / default workspace
    /// context, including a trailing space when non-empty.
    ///
    /// Bare needs `maw exec default -- `; Root runs at the root with no prefix.
    #[must_use]
    pub const fn default_prefix(self) -> &'static str {
        match self {
            Self::Bare => "maw exec default -- ",
            Self::Root => "",
        }
    }

    /// `bn` invocation for the trunk context (`bn` always runs against the trunk).
    #[must_use]
    pub const fn bn_cmd(self) -> &'static str {
        match self {
            Self::Bare => "maw exec default -- bn",
            Self::Root => "bn",
        }
    }

    /// `seal` invocation for the trunk context (e.g. `seal reviews mark-merged`,
    /// which the lead runs against the trunk after a merge).
    #[must_use]
    pub const fn seal_default_cmd(self) -> &'static str {
        match self {
            Self::Bare => "maw exec default -- seal",
            Self::Root => "seal",
        }
    }

    /// Filesystem path to the trunk / default working copy, relative to the
    /// project root.
    #[must_use]
    pub const fn trunk_path(self) -> &'static str {
        match self {
            Self::Bare => "ws/default",
            Self::Root => ".",
        }
    }

    /// Path prefix under which agent workspaces live, with a trailing slash:
    /// `ws/` (bare) or `.maw/workspaces/` (root). Compose with a workspace name,
    /// e.g. `format!("{}{}", layout.ws_prefix(), name)`.
    #[must_use]
    pub const fn ws_prefix(self) -> &'static str {
        match self {
            Self::Bare => "ws/",
            Self::Root => ".maw/workspaces/",
        }
    }

    /// Filesystem path to a named workspace's working directory, relative to the
    /// project root. The trunk (`default`) maps to [`trunk_path`](Self::trunk_path).
    #[must_use]
    pub fn ws_path(self, name: &str) -> String {
        if name == "default" {
            return self.trunk_path().to_string();
        }
        format!("{}{}", self.ws_prefix(), name)
    }

    /// Rewrite a prompt/instruction string authored in **bare** form for the
    /// active layout. A no-op for the bare layout, so bare output is byte-for-byte
    /// unchanged. For the root layout it:
    ///
    /// - drops the `maw exec default -- ` prefix from trunk-context commands
    ///   (the trunk is the repo root); and
    /// - rewrites bare workspace/trunk path tokens (`ws/$WS`, `ws/<ws>`,
    ///   `ws/default/…`) to their root-layout equivalents
    ///   (`.maw/workspaces/$WS`, …, repo-root-relative).
    ///
    /// Order matters: strip the `bn`/`seal` command forms before the generic
    /// prefix so the command name is preserved rather than left with a leading
    /// space, and rewrite the named-workspace tokens before the `ws/default/`
    /// trunk token.
    #[must_use]
    pub fn rewrite_prompt(self, s: String) -> String {
        match self {
            Self::Bare => s,
            Self::Root => s
                // Trunk-context commands lose their prefix.
                .replace("maw exec default -- bn", "bn")
                .replace("maw exec default -- seal", "seal")
                .replace("maw exec default -- ", "")
                // Named-workspace source paths move under .maw/workspaces/.
                .replace("ws/$WS", ".maw/workspaces/$WS")
                .replace("ws/<ws>", ".maw/workspaces/<ws>")
                // The trunk path prefix collapses to the repo root.
                .replace("ws/default/", ""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_bare_when_ws_default_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("ws/default")).unwrap();
        assert_eq!(Layout::detect(dir.path()), Layout::Bare);
    }

    #[test]
    fn detects_bare_from_inside_trunk() {
        // …/ws/default with a sibling-grandparent .manifold dir is the bare trunk.
        let dir = tempfile::tempdir().unwrap();
        let trunk = dir.path().join("ws/default");
        fs::create_dir_all(&trunk).unwrap();
        fs::create_dir_all(dir.path().join(".manifold")).unwrap();
        assert_eq!(Layout::detect(&trunk), Layout::Bare);
    }

    #[test]
    fn root_layout_path_ending_in_ws_default_without_manifold_is_root() {
        // A root-layout checkout that happens to sit at …/ws/default must NOT be
        // misread as bare without the maw `.manifold` marker two levels up.
        let dir = tempfile::tempdir().unwrap();
        let trunk = dir.path().join("ws/default");
        fs::create_dir_all(&trunk).unwrap();
        assert_eq!(Layout::detect(&trunk), Layout::Root);
    }

    #[test]
    fn detects_root_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Layout::detect(dir.path()), Layout::Root);
        // A .maw/workspaces dir is still the root layout.
        fs::create_dir_all(dir.path().join(".maw/workspaces/foo")).unwrap();
        assert_eq!(Layout::detect(dir.path()), Layout::Root);
    }

    #[test]
    fn bare_prefixes_and_paths() {
        let l = Layout::Bare;
        assert_eq!(l.default_prefix(), "maw exec default -- ");
        assert_eq!(l.bn_cmd(), "maw exec default -- bn");
        assert_eq!(l.seal_default_cmd(), "maw exec default -- seal");
        assert_eq!(l.trunk_path(), "ws/default");
        assert_eq!(l.ws_path("alice"), "ws/alice");
        assert_eq!(l.ws_path("default"), "ws/default");
    }

    #[test]
    fn rewrite_prompt_strips_default_prefix_only_for_root() {
        let p = "Run maw exec default -- bn show x then maw exec default -- seal lgtm y and \
                 maw exec default -- git add -A; in workspace use maw exec ws1 -- cargo test"
            .to_string();
        // Bare: unchanged.
        assert_eq!(Layout::Bare.rewrite_prompt(p.clone()), p);
        // Root: trunk prefixes stripped, workspace exec untouched.
        let r = Layout::Root.rewrite_prompt(p);
        assert!(r.contains("Run bn show x then seal lgtm y and git add -A"));
        assert!(r.contains("maw exec ws1 -- cargo test"));
        assert!(!r.contains("maw exec default --"));
    }

    #[test]
    fn rewrite_prompt_rewrites_paths_only_for_root() {
        let p = "edit ws/$WS/src/lib.rs and read ws/<ws>/file; trunk file ws/default/src/main.rs"
            .to_string();
        assert_eq!(Layout::Bare.rewrite_prompt(p.clone()), p);
        let r = Layout::Root.rewrite_prompt(p);
        assert!(r.contains("edit .maw/workspaces/$WS/src/lib.rs"));
        assert!(r.contains("read .maw/workspaces/<ws>/file"));
        assert!(r.contains("trunk file src/main.rs"));
        assert!(!r.contains("ws/default/"));
    }

    #[test]
    fn root_prefixes_and_paths() {
        let l = Layout::Root;
        assert_eq!(l.default_prefix(), "");
        assert_eq!(l.bn_cmd(), "bn");
        assert_eq!(l.seal_default_cmd(), "seal");
        assert_eq!(l.trunk_path(), ".");
        assert_eq!(l.ws_path("alice"), ".maw/workspaces/alice");
        assert_eq!(l.ws_path("default"), ".");
    }
}
