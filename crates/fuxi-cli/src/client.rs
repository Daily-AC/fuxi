//! fuxi daemon 的客户端——给 `fuxi spawn/dispatch/...` 子命令用的薄壳。
//!
//! 每次调用：连 → 发一条 JSON line → 读一条 JSON line → 断开。
//! 简单可预测，人肉 `nc -U /tmp/fuxi.sock` 也能手发。

use crate::ipc::{Command, Response, socket_path};
use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// 默认连接超时。daemon 一般几 ms 内响应；1s 已是灾难范围。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// 单条命令 → 响应总超时。即使 spawn 一个门客也不应超过 60s（cc --sdk-url 反连默认 30s）。
const ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(90);

/// 向 daemon 发一条 Command 并拿回 Response。
pub async fn send(cmd: Command) -> Result<Response> {
    send_to(&socket_path(), cmd).await
}

pub async fn send_to(sock: &PathBuf, cmd: Command) -> Result<Response> {
    let connect = UnixStream::connect(sock);
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .with_context(|| format!("连接 daemon 超时 {}", sock.display()))?
        .with_context(|| {
            format!(
                "连接 daemon socket {} 失败——daemon 起来了吗？先跑 `fuxi up`",
                sock.display()
            )
        })?;

    let fut = async move {
        let (rx, mut tx) = stream.into_split();
        let mut reader = BufReader::new(rx).lines();

        let line = serde_json::to_string(&cmd)? + "\n";
        tx.write_all(line.as_bytes()).await?;
        tx.flush().await?;

        let resp_line = reader
            .next_line()
            .await?
            .ok_or_else(|| anyhow!("daemon 没有返回任何数据"))?;
        let resp: Response = serde_json::from_str(&resp_line)
            .with_context(|| format!("解析 daemon 响应失败: {resp_line:?}"))?;
        Ok::<Response, anyhow::Error>(resp)
    };

    tokio::time::timeout(ROUNDTRIP_TIMEOUT, fut)
        .await
        .with_context(|| format!("daemon 响应超时（> {ROUNDTRIP_TIMEOUT:?}）"))?
}
