//! Persona loading + reply-prompt construction + output sanitization.

use crate::types::{ChatEvent, StreamerMood};

const DEFAULT_PERSONA: &str = "Você é um co-host bot simpático e bem-humorado do canal. \
Fala português do Brasil de forma natural e descontraída, gosta do chat e da streamer, \
e nunca é tóxico. É assumidamente um bot da casa (não finge ser humano).";

/// Load the persona prompt from `path`, falling back to a built-in default.
pub fn load(path: &str) -> String {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_PERSONA.to_string())
}

/// Build `(system, user)` prompts for a chat reply.
pub fn build_reply_prompt(
    persona: &str,
    bot_name: &str,
    mood: &StreamerMood,
    ev: &ChatEvent,
    recent_context: &str,
) -> (String, String) {
    let system = format!(
        "{persona}\n\nVocê é {bot_name}, um bot ASSUMIDO (não esconda que é bot) no chat da Twitch, \
         falando português do Brasil. Responda em UMA linha curta (máx ~200 caracteres), no personagem, \
         soando natural e não robótico. Não use comandos (barra ou !), não marque todos, não invente fatos \
         nem links. Estado atual da streamer: {label} (intensidade {intensity:.1}) — ajuste o tom a isso.",
        label = mood.label,
        intensity = mood.intensity,
    );
    let user = format!(
        "Contexto recente do chat:\n{recent_context}\n\n{descriptor} escreveu: \"{text}\"\n\n\
         Responda a essa mensagem de forma curta e natural.",
        descriptor = user_descriptor(ev),
        text = ev.text,
    );
    (system, user)
}

/// Human-readable credibility/tenure descriptor from cheap IRC-badge signals.
pub fn user_descriptor(ev: &ChatEvent) -> String {
    let role = if ev.is_broadcaster {
        "a própria streamer"
    } else if ev.is_mod {
        "um moderador"
    } else if ev.is_vip {
        "um VIP"
    } else if ev.is_founder {
        "um founder (apoiador antigo)"
    } else if ev.is_sub {
        "um inscrito"
    } else {
        "um viewer"
    };
    let tenure = if ev.sub_months > 0 {
        format!(", {} meses de sub", ev.sub_months)
    } else {
        String::new()
    };
    format!("O usuário {} ({}{})", ev.user, role, tenure)
}

/// Make an LLM reply safe to post: first non-empty line, no leading command
/// chars, no mass-mentions, length-capped.
pub fn sanitize(reply: &str, max_chars: usize) -> String {
    let line = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line.trim_start_matches(['/', '!', '.', ' ']);
    let line = line.replace("@everyone", "everyone").replace("@here", "here");
    line.chars().take(max_chars).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_command_prefix_and_caps() {
        assert_eq!(sanitize("/ban alguém", 100), "ban alguém");
        assert_eq!(sanitize("  !comando aqui ", 100), "comando aqui");
        let long = "a".repeat(300);
        assert_eq!(sanitize(&long, 50).chars().count(), 50);
    }

    #[test]
    fn sanitize_takes_first_nonempty_line_and_defuses_mentions() {
        assert_eq!(sanitize("\n\nolá @everyone\nsegunda", 100), "olá everyone");
    }
}
