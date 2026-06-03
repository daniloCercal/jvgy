//! Pure decision helpers for the interaction engine (mention detection, easter
//! egg roll). Time/RNG are injected so these are deterministically testable.

/// True if `text` addresses the bot by `@login` or as a standalone word.
pub fn is_mention(text: &str, bot_login: &str) -> bool {
    if bot_login.is_empty() {
        return false;
    }
    let t = text.to_lowercase();
    let b = bot_login.to_lowercase();
    if t.contains(&format!("@{b}")) {
        return true;
    }
    t.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == b)
}

/// True if `login` is the easter-egg target and the `roll` (0..1) is under `chance`.
pub fn easter_egg_hit(login: &str, target: &str, chance: f64, roll: f64) -> bool {
    !target.is_empty() && login.eq_ignore_ascii_case(target) && roll < chance
}

/// True if enough time has elapsed since the last proactive message.
pub fn proactive_due(
    now: std::time::Instant,
    last: std::time::Instant,
    min_interval: std::time::Duration,
) -> bool {
    now.duration_since(last) >= min_interval
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mentions() {
        assert!(is_mention("oi @yoaninha_bot tudo bem?", "yoaninha_bot"));
        assert!(is_mention("yoaninha_bot manda ai", "yoaninha_bot"));
        assert!(!is_mention("falando de outra coisa", "yoaninha_bot"));
        assert!(!is_mention("yoaninha_botinho", "yoaninha_bot")); // substring, not a word
        assert!(!is_mention("qualquer coisa", "")); // no bot configured
    }

    #[test]
    fn proactive_respects_min_interval() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        assert!(!proactive_due(t0 + Duration::from_secs(30), t0, Duration::from_secs(120)));
        assert!(proactive_due(t0 + Duration::from_secs(120), t0, Duration::from_secs(120)));
    }

    #[test]
    fn easter_egg_threshold_and_target() {
        assert!(easter_egg_hit("yorods", "yorods", 0.005, 0.004));
        assert!(easter_egg_hit("YoRods", "yorods", 0.005, 0.0)); // case-insensitive
        assert!(!easter_egg_hit("yorods", "yorods", 0.005, 0.006)); // roll too high
        assert!(!easter_egg_hit("alguem", "yorods", 0.005, 0.0)); // wrong user
        assert!(!easter_egg_hit("yorods", "", 0.005, 0.0)); // disabled
    }
}
