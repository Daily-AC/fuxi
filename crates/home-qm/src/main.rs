//! qm — qmledmq.cn 子域名注册表 + Caddyfile 同步 CLI
//!
//! 设计：domains.yaml 单一真相源；`qm sync` 生成 Caddyfile 片段
//! 写到 C:\Caddy\Caddyfile.qm，主 Caddyfile 用 `import` 接入；调
//! caddy.exe reload 让 Caddy 热加载。
//!
//! Tier 模型：
//! - tier1: canonical 核心服务（im / fuxi / sia 等）
//! - tier2: 项目子域名
//! - tier3: lab 实验/agent 临时（路径 <name>.lab.qmledmq.cn，30 天 expire）

use anyhow::{Context, Result, bail};
use chrono::{Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_REGISTRY: &str = r"C:\ProgramData\qm\domains.yaml";
const DEFAULT_OUT: &str = r"C:\Caddy\Caddyfile.qm";
const DEFAULT_MAIN: &str = r"C:\Caddy\Caddyfile";
const DEFAULT_CADDY: &str = r"C:\Caddy\caddy.exe";
const DEFAULT_ADMIN: &str = "localhost:2019";
const EXTERNAL_PORT: u16 = 8443;
const CERT: &str = "C:/Caddy/certs/qmledmq.cn.crt";
const KEY: &str = "C:/Caddy/certs/qmledmq.cn.key";
const TIER3_EXPIRE_DAYS: i64 = 30;

#[derive(Parser, Debug)]
#[command(name = "qm", version, about = "qmledmq.cn 子域名管理 CLI（写 Caddyfile + reload Caddy）", long_about = None)]
struct Cli {
    /// domains.yaml 路径（默认 C:\ProgramData\qm\domains.yaml）
    #[arg(long, global = true, default_value = DEFAULT_REGISTRY)]
    registry: PathBuf,
    /// 生成 Caddyfile 片段输出路径（默认 C:\Caddy\Caddyfile.qm）
    #[arg(long, global = true, default_value = DEFAULT_OUT)]
    out: PathBuf,
    /// 主 Caddyfile（确认 import 指令存在用，sync 时校验）
    #[arg(long, global = true, default_value = DEFAULT_MAIN)]
    main_caddyfile: PathBuf,
    /// caddy.exe 路径
    #[arg(long, global = true, default_value = DEFAULT_CADDY)]
    caddy: PathBuf,
    /// caddy admin API 地址
    #[arg(long, global = true, default_value = DEFAULT_ADMIN)]
    admin: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug, Clone)]
enum Cmd {
    /// 列所有已注册子域名
    List,
    /// 添加一个子域名
    Add {
        /// 子域名段（如 im / fuxi / foo）。tier1/2 = <name>.qmledmq.cn；tier3 = <name>.lab.qmledmq.cn
        name: String,
        /// backend 反代目标（如 localhost:18080 / 192.168.1.20:3000）
        #[arg(long)]
        backend: String,
        /// tier: 1=canonical / 2=project / 3=lab（默认 3）
        #[arg(long, default_value_t = 3)]
        tier: u8,
        /// 用途说明
        #[arg(long, default_value = "")]
        purpose: String,
        /// tier3 默认 30 天 expire，此 flag 禁用 expire
        #[arg(long)]
        no_expire: bool,
    },
    /// 摘除一个子域名（按 name 查 + 删）
    Retire { name: String },
    /// 重新生成 Caddyfile 片段 + reload Caddy
    Sync {
        /// 只生成不 reload（dry-run）
        #[arg(long)]
        no_reload: bool,
    },
    /// 看当前 registry 状态 + reload 命令
    Status,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct Registry {
    #[serde(default)]
    tier1: Vec<Entry>,
    #[serde(default)]
    tier2: Vec<Entry>,
    #[serde(default)]
    tier3: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Entry {
    name: String,
    backend: String,
    #[serde(default)]
    purpose: String,
    #[serde(default)]
    since: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires: Option<String>,
}

fn load_registry(path: &Path) -> Result<Registry> {
    if !path.exists() {
        return Ok(Registry::default());
    }
    let s = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let r: Registry =
        serde_yaml::from_str(&s).with_context(|| format!("parse yaml {}", path.display()))?;
    Ok(r)
}

fn save_registry(path: &Path, r: &Registry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let header = "# qmledmq.cn 域名注册表 - 单一真相源\n# 改完跑 `qm sync` 生成 Caddyfile.qm + reload Caddy\n#\n# Tier 1: canonical 核心服务（命名要好记可分享）\n# Tier 2: projects 项目（codename 也行）\n# Tier 3: lab 实验 / agent 临时（路径 <name>.lab.qmledmq.cn）\n\n";
    let body = serde_yaml::to_string(r).context("serialize yaml")?;
    fs::write(path, format!("{header}{body}"))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn host_for(tier: u8, name: &str) -> String {
    match tier {
        1 | 2 => format!("{name}.qmledmq.cn"),
        3 => format!("{name}.lab.qmledmq.cn"),
        _ => format!("{name}.qmledmq.cn"),
    }
}

fn cmd_list(cli: &Cli) -> Result<()> {
    let r = load_registry(&cli.registry)?;
    for (tier, label, list) in [
        (1u8, "Tier 1 canonical", &r.tier1),
        (2, "Tier 2 project", &r.tier2),
        (3, "Tier 3 lab", &r.tier3),
    ] {
        println!("\n=== {label} ===");
        if list.is_empty() {
            println!("  (empty)");
            continue;
        }
        for e in list {
            let host = host_for(tier, &e.name);
            let exp = e
                .expires
                .as_ref()
                .map(|s| format!("  expires={s}"))
                .unwrap_or_default();
            println!("  {host:<40} -> {:<25}  {}{exp}", e.backend, e.purpose);
        }
    }
    Ok(())
}

fn cmd_add(
    cli: &Cli,
    name: String,
    backend: String,
    tier: u8,
    purpose: String,
    no_expire: bool,
) -> Result<()> {
    if !(1..=3).contains(&tier) {
        bail!("--tier 必须 1 / 2 / 3");
    }
    let mut r = load_registry(&cli.registry)?;
    let exists = match tier {
        1 => r.tier1.iter().any(|e| e.name == name),
        2 => r.tier2.iter().any(|e| e.name == name),
        3 => r.tier3.iter().any(|e| e.name == name),
        _ => false,
    };
    if exists {
        bail!("已存在: {} in tier{tier}", name);
    }
    let today: NaiveDate = Local::now().date_naive();
    let expires = if tier == 3 && !no_expire {
        Some(
            (today + Duration::days(TIER3_EXPIRE_DAYS))
                .format("%Y-%m-%d")
                .to_string(),
        )
    } else {
        None
    };
    let entry = Entry {
        name: name.clone(),
        backend: backend.clone(),
        purpose,
        since: today.format("%Y-%m-%d").to_string(),
        expires,
    };
    match tier {
        1 => r.tier1.push(entry),
        2 => r.tier2.push(entry),
        3 => r.tier3.push(entry),
        _ => unreachable!(),
    }
    save_registry(&cli.registry, &r)?;
    println!(
        "added {} (tier {tier}) -> {}\n→ run: qm sync",
        host_for(tier, &name),
        backend
    );
    Ok(())
}

fn cmd_retire(cli: &Cli, name: String) -> Result<()> {
    let mut r = load_registry(&cli.registry)?;
    let removed_from: Option<&'static str> = {
        let b1 = r.tier1.len();
        r.tier1.retain(|e| e.name != name);
        if r.tier1.len() < b1 {
            Some("tier1")
        } else {
            let b2 = r.tier2.len();
            r.tier2.retain(|e| e.name != name);
            if r.tier2.len() < b2 {
                Some("tier2")
            } else {
                let b3 = r.tier3.len();
                r.tier3.retain(|e| e.name != name);
                if r.tier3.len() < b3 {
                    Some("tier3")
                } else {
                    None
                }
            }
        }
    };
    match removed_from {
        Some(label) => {
            save_registry(&cli.registry, &r)?;
            println!("retired {name} from {label}\n→ run: qm sync");
            Ok(())
        }
        None => bail!("not found: {name}"),
    }
}

fn render_caddyfile(r: &Registry) -> String {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut out = String::new();
    out.push_str(&format!(
        "# AUTO-GENERATED by `qm sync` — DO NOT EDIT MANUALLY\n# Generated at {ts}\n# Edit registry: C:\\ProgramData\\qm\\domains.yaml\n\n"
    ));

    for (tier, label, list) in [
        (1u8, "Tier 1 canonical", &r.tier1),
        (2, "Tier 2 project", &r.tier2),
        (3, "Tier 3 lab", &r.tier3),
    ] {
        if list.is_empty() {
            continue;
        }
        out.push_str(&format!("# === {label} ===\n"));
        for e in list {
            let host = host_for(tier, &e.name);
            let purpose = if e.purpose.is_empty() {
                String::new()
            } else {
                format!("  # {}", e.purpose)
            };
            out.push_str(&format!(
                "\n# {host} -> {}{purpose}\n{host}:{EXTERNAL_PORT} {{\n\ttls {CERT} {KEY}\n\treverse_proxy {}\n}}\n",
                e.backend, e.backend
            ));
        }
        out.push('\n');
    }
    out
}

fn verify_main_import(main_path: &Path, out_path: &Path) -> Result<()> {
    if !main_path.exists() {
        eprintln!(
            "warn: 主 Caddyfile {} 不存在，跳过 import 校验",
            main_path.display()
        );
        return Ok(());
    }
    let s = fs::read_to_string(main_path)?;
    let needle1 = format!("import {}", out_path.display());
    let needle2 = format!(
        "import {}",
        out_path.display().to_string().replace('\\', "/")
    );
    if !s.contains(&needle1) && !s.contains(&needle2) {
        eprintln!(
            "\nwarn: 主 Caddyfile {} 没找到 `import` 指令。\n      加这行到主 Caddyfile（全局/site block 外）：\n        import {}\n",
            main_path.display(),
            out_path.display().to_string().replace('\\', "/")
        );
    }
    Ok(())
}

fn caddy_reload(cli: &Cli) -> Result<()> {
    let output = Command::new(&cli.caddy)
        .args([
            "reload",
            "--config",
            &cli.main_caddyfile.to_string_lossy(),
            "--address",
            &cli.admin,
        ])
        .output()
        .with_context(|| format!("exec {}", cli.caddy.display()))?;
    if !output.status.success() {
        bail!(
            "caddy reload failed (exit {}): stderr={}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    if !out.trim().is_empty() {
        print!("{out}");
    }
    if !err.trim().is_empty() {
        eprint!("{err}");
    }
    Ok(())
}

fn cmd_sync(cli: &Cli, no_reload: bool) -> Result<()> {
    let r = load_registry(&cli.registry)?;
    if let Some(parent) = cli.out.parent() {
        fs::create_dir_all(parent).ok();
    }
    let content = render_caddyfile(&r);
    fs::write(&cli.out, content).with_context(|| format!("write {}", cli.out.display()))?;
    let total = r.tier1.len() + r.tier2.len() + r.tier3.len();
    println!("wrote {} ({total} entries)", cli.out.display());
    verify_main_import(&cli.main_caddyfile, &cli.out)?;
    if no_reload {
        println!("--no-reload 不调 caddy reload");
        return Ok(());
    }
    caddy_reload(cli)?;
    println!("caddy reload OK");
    Ok(())
}

fn cmd_status(cli: &Cli) -> Result<()> {
    let r = load_registry(&cli.registry)?;
    println!("registry: {}", cli.registry.display());
    println!(
        "  tier1={}  tier2={}  tier3={}  total={}",
        r.tier1.len(),
        r.tier2.len(),
        r.tier3.len(),
        r.tier1.len() + r.tier2.len() + r.tier3.len()
    );
    println!("out:      {}", cli.out.display());
    println!("main:     {}", cli.main_caddyfile.display());
    println!("caddy:    {} --address {}", cli.caddy.display(), cli.admin);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.clone() {
        Cmd::List => cmd_list(&cli),
        Cmd::Add {
            name,
            backend,
            tier,
            purpose,
            no_expire,
        } => cmd_add(&cli, name, backend, tier, purpose, no_expire),
        Cmd::Retire { name } => cmd_retire(&cli, name),
        Cmd::Sync { no_reload } => cmd_sync(&cli, no_reload),
        Cmd::Status => cmd_status(&cli),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_for_tiers() {
        assert_eq!(host_for(1, "im"), "im.qmledmq.cn");
        assert_eq!(host_for(2, "blade"), "blade.qmledmq.cn");
        assert_eq!(host_for(3, "foo"), "foo.lab.qmledmq.cn");
    }

    #[test]
    fn render_empty_registry() {
        let r = Registry::default();
        let s = render_caddyfile(&r);
        assert!(s.contains("AUTO-GENERATED"));
        assert!(!s.contains("reverse_proxy"));
    }

    #[test]
    fn render_single_tier1_entry() {
        let r = Registry {
            tier1: vec![Entry {
                name: "im".into(),
                backend: "localhost:18080".into(),
                purpose: "fuxi IM PWA".into(),
                since: "2026-06-30".into(),
                expires: None,
            }],
            ..Default::default()
        };
        let s = render_caddyfile(&r);
        assert!(s.contains("im.qmledmq.cn:8443 {"));
        assert!(s.contains("reverse_proxy localhost:18080"));
        assert!(s.contains("tls C:/Caddy/certs/qmledmq.cn.crt"));
        assert!(s.contains("# fuxi IM PWA"));
    }

    #[test]
    fn render_tier3_uses_lab_subdomain() {
        let r = Registry {
            tier3: vec![Entry {
                name: "foo".into(),
                backend: "localhost:9999".into(),
                purpose: "".into(),
                since: "2026-06-30".into(),
                expires: Some("2026-07-30".into()),
            }],
            ..Default::default()
        };
        let s = render_caddyfile(&r);
        assert!(s.contains("foo.lab.qmledmq.cn:8443 {"));
        assert!(s.contains("reverse_proxy localhost:9999"));
    }

    #[test]
    fn roundtrip_registry() {
        let r = Registry {
            tier1: vec![Entry {
                name: "im".into(),
                backend: "localhost:18080".into(),
                purpose: "fuxi IM".into(),
                since: "2026-06-30".into(),
                expires: None,
            }],
            tier3: vec![Entry {
                name: "foo".into(),
                backend: "localhost:9999".into(),
                purpose: "".into(),
                since: "2026-06-30".into(),
                expires: Some("2026-07-30".into()),
            }],
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&r).unwrap();
        let back: Registry = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.tier1.len(), 1);
        assert_eq!(back.tier3.len(), 1);
        assert_eq!(back.tier1[0].name, "im");
        assert_eq!(back.tier3[0].expires.as_deref(), Some("2026-07-30"));
    }
}
