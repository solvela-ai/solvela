use std::path::{Path, PathBuf};

use thiserror::Error;

/// Supported MCP host applications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    ClaudeCode,
    Cursor,
    ClaudeDesktop,
    OpenClaw,
}

impl Host {
    pub fn from_str(s: &str) -> Result<Self, HostError> {
        match s {
            "claude-code" => Ok(Self::ClaudeCode),
            "cursor" => Ok(Self::Cursor),
            "claude-desktop" => Ok(Self::ClaudeDesktop),
            "openclaw" => Ok(Self::OpenClaw),
            other => Err(HostError::UnknownHost(other.to_owned())),
        }
    }

    /// Whether this host supports --scope (user vs project).
    pub fn supports_scope(&self) -> bool {
        matches!(self, Self::ClaudeCode | Self::Cursor)
    }
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClaudeCode => write!(f, "claude-code"),
            Self::Cursor => write!(f, "cursor"),
            Self::ClaudeDesktop => write!(f, "claude-desktop"),
            Self::OpenClaw => write!(f, "openclaw"),
        }
    }
}

/// Install scope: user-level config vs project-level config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    User,
    Project,
}

impl Scope {
    pub fn from_str(s: &str) -> Result<Self, HostError> {
        match s {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            other => Err(HostError::UnknownScope(other.to_owned())),
        }
    }
}

#[derive(Debug, Error)]
pub enum HostError {
    #[error("unknown host '{0}'; valid values: claude-code, cursor, claude-desktop, openclaw")]
    UnknownHost(String),
    #[error("unknown scope '{0}'; valid values: user, project")]
    UnknownScope(String),
}

/// Resolve the config file path for a given host + scope.
/// Returns `None` for `openclaw` since it delegates to the `openclaw` CLI.
pub fn config_path(host: &Host, scope: &Scope, home: &Path, cwd: &Path) -> Option<PathBuf> {
    match host {
        Host::ClaudeCode => match scope {
            // User scope is handled by shelling out to `claude mcp add --scope user`.
            // There is no single file path to write; return None so callers know to
            // use the CLI path instead.
            Scope::User => None,
            Scope::Project => Some(cwd.join(".mcp.json")),
        },
        Host::Cursor => match scope {
            Scope::User => Some(home.join(".cursor").join("mcp.json")),
            Scope::Project => Some(cwd.join(".cursor").join("mcp.json")),
        },
        Host::ClaudeDesktop => Some(claude_desktop_config_path(home)),
        Host::OpenClaw => None,
    }
}

/// Platform-specific Claude Desktop config path.
pub fn claude_desktop_config_path(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json")
    }
    #[cfg(target_os = "windows")]
    {
        // %APPDATA%\Claude\claude_desktop_config.json
        // On Windows, APPDATA is usually set; fall back to home if not.
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.clone())
            .join("Claude")
            .join("claude_desktop_config.json")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux / other Unix
        home.join(".config")
            .join("Claude")
            .join("claude_desktop_config.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/tmp/home")
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/tmp/project")
    }

    #[test]
    fn test_host_from_str_valid() {
        assert_eq!(Host::from_str("claude-code").unwrap(), Host::ClaudeCode);
        assert_eq!(Host::from_str("cursor").unwrap(), Host::Cursor);
        assert_eq!(
            Host::from_str("claude-desktop").unwrap(),
            Host::ClaudeDesktop
        );
        assert_eq!(Host::from_str("openclaw").unwrap(), Host::OpenClaw);
    }

    #[test]
    fn test_host_from_str_invalid() {
        let err = Host::from_str("vscode").unwrap_err();
        assert!(err.to_string().contains("unknown host"));
        assert!(err.to_string().contains("vscode"));
    }

    #[test]
    fn test_scope_from_str_valid() {
        assert_eq!(Scope::from_str("user").unwrap(), Scope::User);
        assert_eq!(Scope::from_str("project").unwrap(), Scope::Project);
    }

    #[test]
    fn test_scope_from_str_invalid() {
        let err = Scope::from_str("global").unwrap_err();
        assert!(err.to_string().contains("unknown scope"));
    }

    #[test]
    fn test_config_path_claude_code_user() {
        // User scope is delegated to `claude mcp add --scope user`; no file path.
        let p = config_path(&Host::ClaudeCode, &Scope::User, &home(), &cwd());
        assert!(
            p.is_none(),
            "claude-code user scope must return None (CLI-managed)"
        );
    }

    #[test]
    fn test_config_path_claude_code_project() {
        let p = config_path(&Host::ClaudeCode, &Scope::Project, &home(), &cwd()).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/project/.mcp.json"));
    }

    #[test]
    fn test_config_path_cursor_user() {
        let p = config_path(&Host::Cursor, &Scope::User, &home(), &cwd()).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/home/.cursor/mcp.json"));
    }

    #[test]
    fn test_config_path_cursor_project() {
        let p = config_path(&Host::Cursor, &Scope::Project, &home(), &cwd()).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/project/.cursor/mcp.json"));
    }

    #[test]
    fn test_config_path_openclaw_is_none() {
        let p = config_path(&Host::OpenClaw, &Scope::User, &home(), &cwd());
        assert!(p.is_none());
    }

    #[test]
    fn test_claude_desktop_path_current_platform() {
        let p = claude_desktop_config_path(&home());
        let s = p.to_string_lossy();
        // Regardless of platform the filename is always claude_desktop_config.json
        assert!(s.ends_with("claude_desktop_config.json"), "path={s}");
        // And Claude is in the path
        assert!(s.contains("Claude"), "path={s}");
    }

    #[test]
    fn test_host_supports_scope() {
        assert!(Host::ClaudeCode.supports_scope());
        assert!(Host::Cursor.supports_scope());
        assert!(!Host::ClaudeDesktop.supports_scope());
        assert!(!Host::OpenClaw.supports_scope());
    }
}
