//! LLM analysis via an OpenAI-compatible chat-completions endpoint (default MiMo
//! 2.5 Pro). Prompts are incremental: prior summary + new delta only (cost
//! control). Returns the parsed `Insight` plus the estimated USD cost.

use crate::types::Insight;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

pub struct Analyzer {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    input_usd_per_mtok: f64,
    output_usd_per_mtok: f64,
}

impl Analyzer {
    pub fn new(http: reqwest::Client, base_url: String, api_key: String, model: String) -> Self {
        Self {
            http,
            base_url,
            api_key,
            model,
            // MiMo 2.5 Pro list price (<=256K ctx). Override later if needed.
            input_usd_per_mtok: 1.0,
            output_usd_per_mtok: 3.0,
        }
    }

    /// Run one incremental summary. Returns `(insight, estimated_usd)`.
    pub async fn summarize(
        &self,
        channel: &str,
        prior_summary: &str,
        delta: &str,
    ) -> Result<(Insight, f64)> {
        let system = format!(
            "Você é um analista de uma transmissão ao vivo na Twitch do canal {channel}. \
             Você recebe o RESUMO ANTERIOR e NOVO CONTEÚDO (mensagens de chat e falas transcritas). \
             Produza uma análise incremental em português do Brasil. \
             Responda APENAS com JSON válido, sem markdown, no formato exato: \
             {{\"topics\":[\"...\"],\"sentiment\":{{\"mood\":\"...\",\"score\":-1.0}},\
             \"key_moments\":[{{\"t\":\"mm:ss\",\"note\":\"...\"}}],\"running_summary\":\"...\",\
             \"streamer_mood\":{{\"label\":\"neutra|animada|tiltada|cansada|focada\",\"intensity\":0.0}}}}. \
             O campo running_summary deve atualizar e incorporar o resumo anterior, sem repeti-lo por completo. \
             O sentiment é o clima do CHAT; o streamer_mood é o estado da STREAMER inferido das linhas [fala] \
             (intensity de 0.0 a 1.0). O score de sentimento vai de -1.0 (muito negativo) a 1.0 (muito positivo)."
        );
        let prior = if prior_summary.trim().is_empty() {
            "(nenhum ainda)"
        } else {
            prior_summary
        };
        let user = format!("RESUMO ANTERIOR:\n{prior}\n\nNOVO CONTEÚDO:\n{delta}");

        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": 0.3,
            "response_format": {"type": "json_object"}
        });

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp: ChatResp = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("llm request")?
            .error_for_status()
            .context("llm status")?
            .json()
            .await
            .context("llm decode")?;

        let content = resp
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        let usage = resp.usage.unwrap_or_default();
        let cost = (usage.prompt_tokens as f64 / 1_000_000.0) * self.input_usd_per_mtok
            + (usage.completion_tokens as f64 / 1_000_000.0) * self.output_usd_per_mtok;

        let insight = parse_insight(&content).context("parsing llm insight json")?;
        Ok((insight, cost))
    }

    /// Generate a single in-character chat line (plain text, not JSON).
    /// Returns `(text, estimated_usd)`.
    pub async fn reply(&self, system: String, user: String) -> Result<(String, f64)> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": 0.85,
            "max_tokens": 120
        });
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp: ChatResp = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("llm reply request")?
            .error_for_status()
            .context("llm reply status")?
            .json()
            .await
            .context("llm reply decode")?;
        let content = resp
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        let usage = resp.usage.unwrap_or_default();
        let cost = (usage.prompt_tokens as f64 / 1_000_000.0) * self.input_usd_per_mtok
            + (usage.completion_tokens as f64 / 1_000_000.0) * self.output_usd_per_mtok;
        Ok((content, cost))
    }
}

/// Extract the first balanced JSON object from `content` (models sometimes wrap
/// it in prose or markdown fences) and deserialize it into `Insight`.
fn parse_insight(content: &str) -> Result<Insight> {
    let start = content.find('{').context("no json object in llm output")?;
    let end = content.rfind('}').context("no closing brace in llm output")?;
    if end < start {
        anyhow::bail!("malformed json braces in llm output");
    }
    let slice = &content[start..=end];
    let insight: Insight = serde_json::from_str(slice).context("deserializing insight")?;
    Ok(insight)
}

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMsg,
}
#[derive(Deserialize)]
struct ChatMsg {
    #[serde(default)]
    content: String,
}
#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json() {
        let raw = r#"{"topics":["valorant","clutch"],"sentiment":{"mood":"hype","score":0.8},"key_moments":[{"t":"12:30","note":"ace"}],"running_summary":"jogando bem"}"#;
        let i = parse_insight(raw).unwrap();
        assert_eq!(i.topics, vec!["valorant", "clutch"]);
        assert_eq!(i.sentiment.mood, "hype");
        assert_eq!(i.key_moments[0].timestamp, "12:30");
        assert_eq!(i.running_summary, "jogando bem");
    }

    #[test]
    fn parses_json_wrapped_in_prose_and_fences() {
        let raw = "Claro! Aqui está:\n```json\n{\"topics\":[\"x\"],\"running_summary\":\"ok\"}\n```\nespero ajudar";
        let i = parse_insight(raw).unwrap();
        assert_eq!(i.topics, vec!["x"]);
        assert_eq!(i.running_summary, "ok");
    }
}
