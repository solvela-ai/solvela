use serde::{Deserialize, Serialize};

/// Complexity tier assigned by the 15-dimension scorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Simple,
    Medium,
    Complex,
    Reasoning,
}

impl Tier {
    /// Map a raw score to a complexity tier.
    pub fn from_score(score: f64) -> Self {
        if score < 0.0 {
            Tier::Simple
        } else if score < 0.2 {
            Tier::Medium
        } else if score < 0.4 {
            Tier::Complex
        } else {
            Tier::Reasoning
        }
    }
}

/// Routing profile that determines model selection strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Cheapest possible model for each tier.
    Eco,
    /// Balanced cost/quality (default).
    Auto,
    /// Best available model regardless of cost.
    Premium,
    /// Free-tier only models.
    Free,
    /// Tool-use-optimized — prefers models with strong tool/function-calling
    /// fidelity over raw reasoning depth. Activated implicitly when the
    /// inbound `ChatRequest` carries a non-empty `tools` array, or
    /// explicitly via `model: "agentic"` / `model: "blockrun/auto"`.
    ///
    /// Prior art:
    /// - ClawRouter `src/router/rules.ts` — `agenticTask` dimension scoring
    /// - Franklin `src/router/index.ts:174-284` — AGENTIC keyword scoring
    Agentic,
}

impl Profile {
    /// Parse a profile from a string (e.g., model alias).
    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias.to_lowercase().as_str() {
            "eco" | "cheap" | "budget" => Some(Profile::Eco),
            "auto" | "balanced" | "default" => Some(Profile::Auto),
            "premium" | "best" | "quality" => Some(Profile::Premium),
            "free" | "oss" | "open" => Some(Profile::Free),
            // Agentic — explicit aliases. The `blockrun/auto` form is kept
            // for cross-compat with ClawRouter's profile naming.
            "agentic" | "tools" | "tool-use" | "blockrun/auto" => Some(Profile::Agentic),
            _ => None,
        }
    }
}

/// Routing table: maps (Profile, Tier) → model ID.
///
/// Based on the BlockRun ClawRouter routing table, adapted for our
/// Solana-first model catalog.
pub fn resolve_model(profile: Profile, tier: Tier) -> &'static str {
    match (profile, tier) {
        // ECO: cheapest capable model per tier
        (Profile::Eco, Tier::Simple) => "deepseek/deepseek-chat",
        (Profile::Eco, Tier::Medium) => "google/gemini-2.5-flash-lite",
        (Profile::Eco, Tier::Complex) => "deepseek/deepseek-chat",
        (Profile::Eco, Tier::Reasoning) => "deepseek/deepseek-reasoner",

        // AUTO: balanced cost/quality
        (Profile::Auto, Tier::Simple) => "google/gemini-2.5-flash",
        (Profile::Auto, Tier::Medium) => "xai/grok-code-fast-1",
        (Profile::Auto, Tier::Complex) => "google/gemini-3.1-pro",
        (Profile::Auto, Tier::Reasoning) => "xai/grok-4-fast-reasoning",

        // PREMIUM: best quality regardless of cost
        (Profile::Premium, Tier::Simple) => "openai/gpt-4o",
        (Profile::Premium, Tier::Medium) => "anthropic/claude-sonnet-4-20250514",
        (Profile::Premium, Tier::Complex) => "anthropic/claude-opus-4-20250514",
        (Profile::Premium, Tier::Reasoning) => "openai/o3",

        // FREE: only free-tier models
        (Profile::Free, Tier::Simple) => "openai/gpt-oss-120b",
        (Profile::Free, Tier::Medium) => "openai/gpt-oss-120b",
        (Profile::Free, Tier::Complex) => "openai/gpt-oss-120b",
        (Profile::Free, Tier::Reasoning) => "openai/gpt-oss-120b",

        // AGENTIC: prefer models with strong tool-calling fidelity over
        // raw reasoning depth. Prior art:
        // - ClawRouter `src/router/rules.ts` `agenticTask` dimension
        // - Franklin `src/router/index.ts:174-284` AGENTIC keyword scoring
        // The picks below all have `supports_tools = true` in
        // `config/models.toml` and are well-tested for tool/function calls.
        (Profile::Agentic, Tier::Simple) => "openai/gpt-4o-mini",
        (Profile::Agentic, Tier::Medium) => "openai/gpt-4.1",
        (Profile::Agentic, Tier::Complex) => "anthropic/claude-sonnet-4-20250514",
        (Profile::Agentic, Tier::Reasoning) => "anthropic/claude-opus-4-20250514",
    }
}

/// Model alias resolution: maps shorthand names to canonical model IDs.
pub fn resolve_alias(alias: &str) -> Option<&'static str> {
    match alias.to_lowercase().as_str() {
        "gpt5" | "gpt-5" => Some("openai/gpt-5.2"),
        "sonnet" | "claude-sonnet" => Some("anthropic/claude-sonnet-4-20250514"),
        "opus" | "claude-opus" => Some("anthropic/claude-opus-4-20250514"),
        "haiku" | "claude-haiku" => Some("anthropic/claude-3-5-haiku-20241022"),
        "gemini" | "gemini-pro" => Some("google/gemini-3.1-pro"),
        "flash" | "gemini-flash" => Some("google/gemini-2.5-flash"),
        "grok" | "grok-fast" => Some("xai/grok-4-fast-reasoning"),
        "deepseek" | "ds" => Some("deepseek/deepseek-chat"),
        "deepseek-r" | "reasoner" => Some("deepseek/deepseek-reasoner"),
        "free" | "oss" => Some("openai/gpt-oss-120b"),
        "demo" | "echo" => Some("solvela/demo"),
        "o3-mini" | "o3mini" => Some("openai/o3-mini"),
        "o4-mini" | "o4mini" => Some("openai/o4-mini"),
        "gpt4.1" | "gpt-4.1" | "gpt41" => Some("openai/gpt-4.1"),
        "gpt4.1-mini" | "gpt-4.1-mini" => Some("openai/gpt-4.1-mini"),
        "gpt4.1-nano" | "gpt-4.1-nano" => Some("openai/gpt-4.1-nano"),
        "sonnet4.5" | "sonnet-4.5" => Some("anthropic/claude-sonnet-4-20250514"),
        "grok3" | "grok-3" => Some("xai/grok-3"),
        "grok3-mini" | "grok-3-mini" => Some("xai/grok-3-mini"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_boundaries() {
        assert_eq!(Tier::from_score(-0.5), Tier::Simple);
        assert_eq!(Tier::from_score(-0.01), Tier::Simple);
        assert_eq!(Tier::from_score(0.0), Tier::Medium);
        assert_eq!(Tier::from_score(0.19), Tier::Medium);
        assert_eq!(Tier::from_score(0.2), Tier::Complex);
        assert_eq!(Tier::from_score(0.39), Tier::Complex);
        assert_eq!(Tier::from_score(0.4), Tier::Reasoning);
        assert_eq!(Tier::from_score(1.0), Tier::Reasoning);
    }

    #[test]
    fn test_profile_from_alias() {
        assert_eq!(Profile::from_alias("eco"), Some(Profile::Eco));
        assert_eq!(Profile::from_alias("AUTO"), Some(Profile::Auto));
        assert_eq!(Profile::from_alias("premium"), Some(Profile::Premium));
        assert_eq!(Profile::from_alias("free"), Some(Profile::Free));
        assert_eq!(Profile::from_alias("unknown"), None);
    }

    #[test]
    fn test_resolve_model() {
        assert_eq!(
            resolve_model(Profile::Free, Tier::Reasoning),
            "openai/gpt-oss-120b"
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Reasoning),
            "openai/o3"
        );
        assert_eq!(
            resolve_model(Profile::Auto, Tier::Simple),
            "google/gemini-2.5-flash"
        );
    }

    #[test]
    fn test_resolve_alias() {
        assert_eq!(resolve_alias("gpt5"), Some("openai/gpt-5.2"));
        assert_eq!(
            resolve_alias("sonnet"),
            Some("anthropic/claude-sonnet-4-20250514")
        );
        assert_eq!(resolve_alias("nonexistent"), None);
    }

    #[test]
    fn test_agentic_profile_recognized_aliases() {
        assert_eq!(Profile::from_alias("agentic"), Some(Profile::Agentic));
        assert_eq!(Profile::from_alias("AGENTIC"), Some(Profile::Agentic));
        assert_eq!(Profile::from_alias("tools"), Some(Profile::Agentic));
        assert_eq!(Profile::from_alias("tool-use"), Some(Profile::Agentic));
        assert_eq!(Profile::from_alias("blockrun/auto"), Some(Profile::Agentic));
    }

    #[test]
    fn test_agentic_profile_resolves_to_tool_capable_models() {
        // Every tier in the agentic profile should resolve to a model that
        // appears in `config/models.toml` with `supports_tools = true`.
        // We don't load the registry here (router crate is independent) —
        // these IDs are checked by the gateway's integration tests which
        // load the real TOML.
        assert_eq!(
            resolve_model(Profile::Agentic, Tier::Simple),
            "openai/gpt-4o-mini"
        );
        assert_eq!(
            resolve_model(Profile::Agentic, Tier::Medium),
            "openai/gpt-4.1"
        );
        assert_eq!(
            resolve_model(Profile::Agentic, Tier::Complex),
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(
            resolve_model(Profile::Agentic, Tier::Reasoning),
            "anthropic/claude-opus-4-20250514"
        );
    }

    #[test]
    fn test_agentic_profile_differs_from_auto() {
        // Agentic and Auto must produce different selections at every tier
        // (otherwise the agentic profile is just a synonym for auto).
        for tier in [Tier::Simple, Tier::Medium, Tier::Complex, Tier::Reasoning] {
            let auto = resolve_model(Profile::Auto, tier);
            let agentic = resolve_model(Profile::Agentic, tier);
            assert_ne!(
                auto, agentic,
                "agentic and auto should pick different models at tier {tier:?}"
            );
        }
    }
}
