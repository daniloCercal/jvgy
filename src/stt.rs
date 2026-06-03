//! Pluggable speech-to-text. Default `mimo` (MiMo-V2.5-Pro audio understanding,
//! uses prepaid tokens); `openai` (gpt-4o-mini-transcribe) and `deepgram` are
//! paid fallbacks. Provider is selected at runtime via enum dispatch (no async
//! trait objects). All providers take a raw PCM s16le 16 kHz mono buffer.
//!
//! NOTE: the MiMo audio request follows the OpenAI-compatible `input_audio`
//! convention; the exact schema/quality for PT-BR must be confirmed by the
//! Phase-2 pre-flight test (design s6a) before trusting it as the default.

use crate::audio::wav;
use crate::util::base64_encode;
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone)]
pub enum SttProvider {
    Mimo {
        base_url: String,
        api_key: String,
        model: String,
    },
    OpenAi {
        base_url: String,
        api_key: String,
        model: String,
    },
    Deepgram {
        api_key: String,
        model: String,
        language: String,
    },
}

impl SttProvider {
    pub fn from_config(
        provider: &str,
        base_url: &str,
        api_key: &str,
        model: &str,
        language: &str,
    ) -> Self {
        match provider {
            "openai" => SttProvider::OpenAi {
                base_url: base_url.into(),
                api_key: api_key.into(),
                model: model.into(),
            },
            "deepgram" => SttProvider::Deepgram {
                api_key: api_key.into(),
                model: if model.is_empty() { "nova-3".into() } else { model.into() },
                language: language.into(),
            },
            _ => SttProvider::Mimo {
                base_url: base_url.into(),
                api_key: api_key.into(),
                model: model.into(),
            },
        }
    }

    /// Approx USD per minute of audio for the cost governor. MiMo bills prepaid
    /// tokens, so marginal cash cost is treated as ~0.
    pub fn cost_per_min(&self) -> f64 {
        match self {
            SttProvider::Mimo { .. } => 0.0,
            SttProvider::OpenAi { .. } => 0.003,
            SttProvider::Deepgram { .. } => 0.0043,
        }
    }

    pub async fn transcribe(&self, http: &reqwest::Client, pcm_s16le: &[u8]) -> Result<String> {
        let wav = wav::wav_from_pcm_s16le(pcm_s16le, 16_000, 1);
        match self {
            SttProvider::Mimo {
                base_url,
                api_key,
                model,
            } => {
                let b64 = base64_encode(&wav);
                let body = serde_json::json!({
                    "model": model,
                    "messages": [{
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "Transcreva o áudio em português. Responda apenas com a transcrição, sem comentários."},
                            {"type": "input_audio", "input_audio": {"data": b64, "format": "wav"}}
                        ]
                    }],
                    "temperature": 0.0
                });
                let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                let v: ChatResp = http
                    .post(url)
                    .bearer_auth(api_key)
                    .json(&body)
                    .send()
                    .await
                    .context("mimo stt request")?
                    .error_for_status()
                    .context("mimo stt status")?
                    .json()
                    .await
                    .context("mimo stt decode")?;
                Ok(v.choices
                    .into_iter()
                    .next()
                    .map(|c| c.message.content)
                    .unwrap_or_default())
            }
            SttProvider::OpenAi {
                base_url,
                api_key,
                model,
            } => {
                let part = reqwest::multipart::Part::bytes(wav)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?;
                let form = reqwest::multipart::Form::new()
                    .text("model", model.clone())
                    .text("language", "pt")
                    .part("file", part);
                let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
                let v: TextResp = http
                    .post(url)
                    .bearer_auth(api_key)
                    .multipart(form)
                    .send()
                    .await
                    .context("openai stt request")?
                    .error_for_status()
                    .context("openai stt status")?
                    .json()
                    .await
                    .context("openai stt decode")?;
                Ok(v.text)
            }
            SttProvider::Deepgram {
                api_key,
                model,
                language,
            } => {
                let v: DeepgramResp = http
                    .post("https://api.deepgram.com/v1/listen")
                    .query(&[
                        ("model", model.as_str()),
                        ("language", language.as_str()),
                        ("punctuate", "true"),
                    ])
                    .header("Authorization", format!("Token {api_key}"))
                    .header("Content-Type", "audio/wav")
                    .body(wav)
                    .send()
                    .await
                    .context("deepgram request")?
                    .error_for_status()
                    .context("deepgram status")?
                    .json()
                    .await
                    .context("deepgram decode")?;
                Ok(v.results
                    .channels
                    .into_iter()
                    .next()
                    .and_then(|c| c.alternatives.into_iter().next())
                    .map(|a| a.transcript)
                    .unwrap_or_default())
            }
        }
    }
}

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<ChatChoice>,
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

#[derive(Deserialize)]
struct TextResp {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct DeepgramResp {
    results: DgResults,
}
#[derive(Deserialize)]
struct DgResults {
    channels: Vec<DgChannel>,
}
#[derive(Deserialize)]
struct DgChannel {
    alternatives: Vec<DgAlt>,
}
#[derive(Deserialize)]
struct DgAlt {
    #[serde(default)]
    transcript: String,
}
