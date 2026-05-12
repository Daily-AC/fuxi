//! Phase 5-B SV：wake 命中后调 sv_server `/verify` 拒陌生人。
//!
//! 用一段 ~3s 的 PCM ring buffer（命中时刻往前数 3s 的音频）→ wav bytes →
//! POST `<sv_base>/verify`，依赖 sv_server 返 `{match, score, threshold, enrolled}`。
//! `match=false` 时 wake event 静默丢——客户端完全感知不到唤醒（玄女不说"我在"）。
//!
//! 为啥不依赖 `fuxi-im` crate 自签 token：wake-server 部署成独立 systemd 单元，
//! 二进制大小 + 依赖图越小越稳；复制 30 行 HMAC-SHA256 签名逻辑成本远低于拉
//! 整个 fuxi-im 依赖（含 axum 路由、错误类型、数据库 schema 等无关代码）。

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// SV verify 客户端。AppState 持 `Arc<SvClient>`；wake_loop 命中后调 `verify`。
pub struct SvClient {
    base_url: String,
    hmac_secret: Vec<u8>,
    http: reqwest::Client,
}

#[derive(Debug, serde::Deserialize)]
pub struct VerifyResult {
    #[serde(rename = "match")]
    pub matched: bool,
    pub score: f64,
    pub threshold: f64,
    pub enrolled: bool,
}

impl SvClient {
    /// `base_url` 例 `http://127.0.0.1:9883`；`hmac_secret` = `~/.fuxi/im_hmac.key`
    /// 内容（trim 后），需要跟 sv_server.py 用同一份才能验签通过。
    pub fn new(base_url: String, hmac_secret: Vec<u8>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest::Client 构建");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            hmac_secret,
            http,
        }
    }

    /// 从 HMAC key 文件加载（同 fuxi-im 路径）。`im_hmac.key` 是 0600 文件，
    /// systemd 跑的 fuxi-wake.service 必须以 User=e0-7 跑能读才行。
    pub fn from_key_file(base_url: String, key_path: &std::path::Path) -> Result<Self> {
        let key = std::fs::read(key_path)
            .with_context(|| format!("读 HMAC key {} 失败", key_path.display()))?;
        let key: Vec<u8> = key
            .into_iter()
            .filter(|&b| b != b'\n' && b != b'\r' && b != b' ')
            .collect();
        if key.is_empty() {
            anyhow::bail!("HMAC key 文件 {} 为空", key_path.display());
        }
        Ok(Self::new(base_url, key))
    }

    /// 自签一颗 60s 短 token——每次 verify 调用前 mint，避免维护 token TTL。
    fn mint_token(&self) -> Result<String> {
        let claims = serde_json::json!({
            "device_id": format!("wake-sv-{}", uuid::Uuid::new_v4()),
            "name": "wake-sv",
            "expires_at": (chrono::Utc::now() + chrono::Duration::seconds(60))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
        let body = serde_json::to_vec(&claims)?;
        let mut mac = HmacSha256::new_from_slice(&self.hmac_secret)
            .expect("HMAC-SHA256 接受任意长度 key");
        mac.update(&body);
        let sig = mac.finalize().into_bytes();
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&body),
            URL_SAFE_NO_PAD.encode(sig)
        ))
    }

    /// PCM int16 mono 16kHz → wav bytes（in-memory）→ POST /verify。
    /// 失败（网络 / 5xx / 解析）返 Err，由调用方决定如何降级（推荐 fail-open 放行）。
    pub async fn verify_pcm_i16(&self, pcm: &[i16]) -> Result<VerifyResult> {
        let wav = pcm_i16_to_wav_bytes(pcm, 16000);
        let wav_b64 = B64.encode(&wav);
        let token = self.mint_token()?;
        let url = format!("{}/verify", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({ "wav_b64": wav_b64 }))
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("sv verify {} → {}: {}", url, status, text);
        }
        let parsed: VerifyResult = serde_json::from_str(&text)
            .with_context(|| format!("sv verify 返非预期 JSON: {text}"))?;
        debug!(score = parsed.score, matched = parsed.matched, enrolled = parsed.enrolled, "sv verify");
        Ok(parsed)
    }
}

/// 拼一个最小 PCM_16 WAV header + raw samples 直接 in-memory。
/// 不用 hound 是为了避免再加一个 dep——header 44 字节固定，写 25 行就行。
fn pcm_i16_to_wav_bytes(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let num_samples = pcm.len() as u32;
    let data_size = num_samples * 2; // 16-bit mono
    let mut out = Vec::with_capacity(44 + data_size as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // format = PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // channels = 1
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    for &s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// 3s ring buffer for 16kHz mono i16 PCM = 48000 samples。
/// wake 触发瞬间含「玄女」唤醒词的前后约 1-2s，3s 容量足够覆盖。
pub struct PcmRing {
    buf: std::collections::VecDeque<i16>,
    cap: usize,
}

impl PcmRing {
    pub fn new(capacity_samples: usize) -> Self {
        Self {
            buf: std::collections::VecDeque::with_capacity(capacity_samples),
            cap: capacity_samples,
        }
    }

    /// 喂 PCM byte（int16 LE mono）—— 解析进 ring，超过容量丢前面。
    pub fn push_pcm_bytes(&mut self, bytes: &[u8]) {
        // 偶数 chunk 处理；奇数末尾字节丢（应不会，客户端按 chunk size 控制）
        let usable = bytes.len() & !1;
        for chunk in bytes[..usable].chunks_exact(2) {
            let s = i16::from_le_bytes([chunk[0], chunk[1]]);
            if self.buf.len() == self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(s);
        }
    }

    /// 取当前 ring 内容快照（拷贝）。
    pub fn snapshot(&self) -> Vec<i16> {
        self.buf.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// AppState 持的 SV 配置：optional —— 不配置时 wake_loop 完全不调 SV，行为
/// 跟 Phase 5-B 前一致（任何人喊「玄女」都触发）。
#[derive(Clone)]
pub struct SvConfig {
    pub client: Arc<SvClient>,
}

impl SvConfig {
    /// 命中时调用一次。返：是否放行 wake event。
    /// fail-open：SV 不通时放行（不要因为旁路故障让用户喊不动玄女）。
    pub async fn should_emit_wake(&self, pcm: &[i16], client_id: &str) -> bool {
        if pcm.is_empty() {
            warn!(%client_id, "wake sv: ring 为空（fail-open 放行）");
            return true;
        }
        match self.client.verify_pcm_i16(pcm).await {
            Ok(r) if !r.enrolled => {
                debug!(%client_id, "wake sv: 未注册 owner（fail-open 放行）");
                true
            }
            Ok(r) if r.matched => {
                info!(%client_id, score = r.score, "wake sv: 主人 ✓ 放行");
                true
            }
            Ok(r) => {
                info!(
                    %client_id,
                    score = r.score, threshold = r.threshold,
                    "wake sv: 非主人 ✗ 拒（静默丢 wake event）"
                );
                false
            }
            Err(e) => {
                warn!(%client_id, error = ?e, "wake sv: 调用失败（fail-open 放行）");
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_44_bytes_and_riff() {
        let pcm = vec![0i16; 1600]; // 100ms @ 16kHz
        let wav = pcm_i16_to_wav_bytes(&pcm, 16000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // file size = 44 header + 2 * 1600 samples = 3244 bytes
        assert_eq!(wav.len(), 44 + 1600 * 2);
        // RIFF size = total - 8
        let riff_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        assert_eq!(riff_size as usize, wav.len() - 8);
    }

    #[test]
    fn ring_evicts_oldest_when_full() {
        let mut r = PcmRing::new(4);
        // 4 个 i16 = 8 bytes
        r.push_pcm_bytes(&[1, 0, 2, 0, 3, 0, 4, 0]); // [1,2,3,4]
        assert_eq!(r.snapshot(), vec![1, 2, 3, 4]);
        r.push_pcm_bytes(&[5, 0, 6, 0]); // [3,4,5,6]
        assert_eq!(r.snapshot(), vec![3, 4, 5, 6]);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn token_format_two_parts_b64url() {
        let c = SvClient::new("http://127.0.0.1:9883".into(), b"test-secret".to_vec());
        let t = c.mint_token().expect("mint");
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 2, "token = body.sig");
        // url-safe base64 no padding
        assert!(!t.contains('='));
        assert!(!t.contains('+'));
        assert!(!t.contains('/'));
    }
}
