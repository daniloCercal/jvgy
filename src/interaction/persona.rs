//! Persona loading + reply-prompt construction + output sanitization.

use crate::interaction::registry::{credibility, UserProfile};
use crate::types::StreamerMood;

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

fn base_system(persona: &str, bot_name: &str, mood: &StreamerMood) -> String {
    format!(
        "{persona}\n\nVocê é {bot_name}, um bot ASSUMIDO (não esconda que é bot) no chat da Twitch, \
         falando português do Brasil. Escreva UMA linha curta (máx ~200 caracteres), no personagem, \
         soando natural e não robótico. Não use comandos (barra ou !), não marque todos, não invente \
         fatos nem links. Estado atual da streamer: {label} (intensidade {intensity:.1}) — ajuste o tom.",
        label = mood.label,
        intensity = mood.intensity,
    )
}

/// `(system, user)` prompts to reply to a specific message.
pub fn build_reply_prompt(
    persona: &str,
    bot_name: &str,
    mood: &StreamerMood,
    user_desc: &str,
    trigger_text: &str,
    recent_context: &str,
) -> (String, String) {
    let user = format!(
        "Contexto recente do chat:\n{recent_context}\n\n{user_desc} escreveu: \"{trigger_text}\"\n\n\
         Responda a essa mensagem de forma curta e natural.",
    );
    (base_system(persona, bot_name, mood), user)
}

/// `(system, user)` prompts to proactively join the conversation (no specific
/// person; only when there is something worth saying).
pub fn build_proactive_prompt(
    persona: &str,
    bot_name: &str,
    mood: &StreamerMood,
    recent_context: &str,
) -> (String, String) {
    let user = format!(
        "Contexto recente do chat:\n{recent_context}\n\n\
         Mande UMA mensagem curta participando do papo de forma natural, sem responder a ninguém \
         específico e sem forçar. Se não houver nada relevante a dizer, seja breve.",
    );
    (base_system(persona, bot_name, mood), user)
}

/// Human-readable credibility/tenure descriptor from a registry profile.
pub fn describe_user(p: &UserProfile) -> String {
    let cred = credibility(p);
    let mut bits = vec![cred.tier.to_string()];
    if p.sub_months > 0 {
        bits.push(format!("{} meses de sub", p.sub_months));
    }
    if let Some(days) = p.account_age_days {
        let years = days / 365;
        if years >= 1 {
            bits.push(format!("conta de ~{years} ano(s)"));
        } else {
            bits.push(format!("conta de {days} dias (nova)"));
        }
    }
    format!("O usuário {} ({})", p.display, bits.join(", "))
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
