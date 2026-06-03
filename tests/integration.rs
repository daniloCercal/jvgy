//! Integration tests against MOCKED providers. A tiny in-process HTTP server
//! returns canned JSON so we exercise the real reqwest client + response parsing
//! without any network, API keys, ffmpeg, or a live Twitch channel.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use twitch_discord_summarizer::analysis::Analyzer;
use twitch_discord_summarizer::stt::SttProvider;

/// Serve exactly one HTTP request with the given JSON body, then close.
/// Returns the base URL (e.g. `http://127.0.0.1:54321`).
async fn serve_once(body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            // Read until end of request headers (we don't need the body).
            let mut data = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                match sock.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        data.extend_from_slice(&tmp[..n]);
                        if data.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn llm_summarize_parses_insight_and_computes_cost() {
    // The model returns the Insight JSON as the message content.
    let content = serde_json::json!({
        "topics": ["valorant", "clutch"],
        "sentiment": {"mood": "hype", "score": 0.7},
        "key_moments": [{"t": "10:00", "note": "ace"}],
        "running_summary": "stream animada"
    })
    .to_string();
    let body = serde_json::json!({
        "choices": [{"message": {"content": content}}],
        "usage": {"prompt_tokens": 1000, "completion_tokens": 200}
    })
    .to_string();

    let url = serve_once(body).await;
    let analyzer = Analyzer::new(reqwest::Client::new(), url, "test-key".into(), "mimo".into());

    let (insight, cost) = analyzer
        .summarize("gaules", "", "[chat] alice: ola")
        .await
        .expect("summarize should succeed against mock");

    assert_eq!(insight.topics, vec!["valorant", "clutch"]);
    assert_eq!(insight.sentiment.mood, "hype");
    assert_eq!(insight.running_summary, "stream animada");
    // 1000/1e6 * $1 + 200/1e6 * $3 = 0.001 + 0.0006
    assert!((cost - 0.0016).abs() < 1e-9, "unexpected cost {cost}");
}

#[tokio::test]
async fn mimo_stt_returns_transcript() {
    let body = serde_json::json!({
        "choices": [{"message": {"content": "olá mundo isto é um teste"}}]
    })
    .to_string();

    let url = serve_once(body).await;
    let provider = SttProvider::Mimo {
        base_url: url,
        api_key: "test-key".into(),
        model: "mimo-v2.5-pro".into(),
    };

    let pcm = vec![0u8; 640]; // tiny silent buffer; VAD isn't involved here
    let text = provider
        .transcribe(&reqwest::Client::new(), &pcm)
        .await
        .expect("transcribe should succeed against mock");

    assert_eq!(text, "olá mundo isto é um teste");
}
