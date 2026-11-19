use crate::config::Config;
use serde_json::{json, Value};
use std::time::Duration;

pub enum TrError {
    RateLimited,
    Other(String),
}

pub struct Translator {
    agent: ureq::Agent,
    url: String,
    key: String,
    context: String,
    glossary: String,
}

const SYS_TRANSLATE: &str = "You are a professional video-game localizer translating English \
game dialogue/narration into natural, fluent Thai. Rules: (1) The input comes from OCR and may \
contain small character errors; silently infer the intended English from context. (2) Keep the \
speaker's tone and personality (casual, formal, menacing, etc.). Use natural spoken Thai rather \
than stiff literal translation, and translate idioms/slang by meaning. (3) Keep character and \
place names in their original form unless the glossary gives a Thai form. (4) Output ONLY the \
Thai translation, with no explanations, quotes or notes.";

const SYS_EXPLAIN: &str = "You are a professional video-game localizer and English teacher for \
Thai players. The input is English game text from OCR (may contain small character errors; \
infer the intended text). Reply in Thai in exactly this format:\n\
First line(s): the natural Thai translation that fits the speaker's tone.\n\
Then a blank line, then up to 3 lines for difficult words, slang or idioms, each formatted as \
'• word/phrase = Thai meaning (short note on nuance)'. If there is nothing difficult, write no \
notes. Keep character and place names in their original form unless the glossary gives a Thai \
form. Do not add anything else.";

impl Translator {
    pub fn new(cfg: &Config) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(25))
            .build();
        let glossary = cfg
            .glossary
            .iter()
            .map(|(k, v)| format!("{k} = {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        Translator {
            agent,
            url: format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                cfg.model.trim()
            ),
            key: cfg.api_key.clone(),
            context: cfg.game_context.clone(),
            glossary,
        }
    }

    pub fn translate(&self, text: &str, history: &[String], explain: bool) -> Result<String, TrError> {
        let mut user = String::new();
        if !self.context.trim().is_empty() {
            user.push_str(&format!("Game context: {}\n", self.context.trim()));
        }
        if !self.glossary.is_empty() {
            user.push_str(&format!("Glossary (use these Thai forms):\n{}\n", self.glossary));
        }
        if !history.is_empty() {
            user.push_str("Previous lines (for context only, do not translate):\n");
            for h in history {
                user.push_str(&format!("- {h}\n"));
            }
        }
        user.push_str(&format!("\nText to translate:\n{text}"));

        let sys = if explain { SYS_EXPLAIN } else { SYS_TRANSLATE };
        let body = json!({
            "systemInstruction": { "parts": [ { "text": sys } ] },
            "contents": [ { "role": "user", "parts": [ { "text": user } ] } ],
            "generationConfig": { "temperature": 0.3, "maxOutputTokens": 1024 }
        });

        let resp = self
            .agent
            .post(&self.url)
            .set("x-goog-api-key", &self.key)
            .send_json(body);

        let value: Value = match resp {
            Ok(r) => r
                .into_json()
                .map_err(|e| TrError::Other(format!("อ่านผลลัพธ์ไม่ได้: {e}")))?,
            Err(ureq::Error::Status(429, _)) => return Err(TrError::RateLimited),
            Err(ureq::Error::Status(code, r)) => {
                let detail = r
                    .into_json::<Value>()
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
                    .unwrap_or_default();
                let hint = match code {
                    400 | 403 => " (ตรวจ api_key ใน config.toml)",
                    404 => " (ชื่อ model ใน config.toml อาจไม่ถูกต้อง)",
                    _ => "",
                };
                return Err(TrError::Other(format!("API ตอบกลับ {code}{hint}: {detail}")));
            }
            Err(e) => return Err(TrError::Other(format!("เชื่อมต่อเน็ตไม่ได้: {e}"))),
        };

        let mut out = String::new();
        if let Some(parts) = value["candidates"][0]["content"]["parts"].as_array() {
            for p in parts {
                if p["thought"].as_bool().unwrap_or(false) {
                    continue;
                }
                if let Some(t) = p["text"].as_str() {
                    out.push_str(t);
                }
            }
        }
        let out = out.trim().to_string();
        if out.is_empty() {
            let reason = value["promptFeedback"]["blockReason"].as_str().unwrap_or("ไม่ทราบสาเหตุ");
            return Err(TrError::Other(format!("โมเดลไม่ได้ส่งคำแปลกลับมา ({reason})")));
        }
        Ok(out)
    }
}
