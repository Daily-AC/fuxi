//! `fuxi profile` —— 玄女维护用户身份卡的子命令。
//!
//! 设计跟 `memory_cmd.rs` 同 spirit：直接对 SQLite 文件做 `UserProfileStore` 操作，
//! 不走 daemon。db 路径默认 `$HOME/.fuxi/events.db`——同 `fuxi note` 那一份共用库
//! （memory + events 同库是仓颉/玄女整套策府演进路线，详见 architecture-v1）。
//!
//! 子命令：
//! - `fuxi profile set <key> <value> [--source <s>]`
//! - `fuxi profile get <key>`
//! - `fuxi profile list`
//! - `fuxi profile unset <key>`
//!
//! 跟 `fuxi memory record` 区别（这是玄女最易混的点）：
//! - `memory record` 写**事实三元组**（subject/predicate/object，零碎）
//! - `profile set`  写**身份卡条目**（key/value，凝练，spawn 时整段注入门客 prompt）
//!
//! 论文 arXiv:2604.14004 结论：trajectory 层（事实流）会 negative transfer，
//! summary 层（身份卡）才适合迁移。所以"用户是谁、约定是啥"走 profile 不走 memory。

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_memory::{NewProfile, UserProfileStore};
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct ProfileArgs {
    /// SQLite 路径覆写。省略走 `$HOME/.fuxi/events.db`——同 `fuxi note` / fuxi-im 那一份。
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: ProfileCmd,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCmd {
    /// 写一条身份卡（同 key 多写都会变 active 多行；要替换走 `unset` 后重 `set`，
    /// 或调底层 `UserProfileStore::supersede`——CLI 暂不暴露 supersede，由仓颉走）。
    Set(SetArgs),
    /// 取某 key 的当前活值。
    Get(GetArgs),
    /// 列出所有活行。
    List,
    /// 把 key 的所有活行标过期（不真删，valid_until=now）。
    Unset(UnsetArgs),
}

#[derive(Debug, ClapArgs)]
pub struct SetArgs {
    /// 身份卡 key（如 `identity` / `tone` / `tech_stack`）。**禁空格**——空格易被
    /// shell 拆参数让人晕。多 token 用下划线。
    pub key: String,
    /// 身份卡 value（短句最佳；总 summary 受 200 字截断约束）。
    pub value: String,
    /// 来源标记。默认 `xuannv-explicit`——表示玄女在对话里明确判定该入卡
    /// （区别于将来仓颉 auto-extract 的 `cangjie-auto`）。
    #[arg(long, default_value = "xuannv-explicit")]
    pub source: String,
}

#[derive(Debug, ClapArgs)]
pub struct GetArgs {
    pub key: String,
}

#[derive(Debug, ClapArgs)]
pub struct UnsetArgs {
    pub key: String,
}

pub async fn run(args: ProfileArgs) -> Result<()> {
    let db = resolve_db_path(args.db)?;
    match args.cmd {
        ProfileCmd::Set(a) => run_set(&db, a).await,
        ProfileCmd::Get(a) => run_get(&db, a).await,
        ProfileCmd::List => run_list(&db).await,
        ProfileCmd::Unset(a) => run_unset(&db, a).await,
    }
}

/// `--db` > `$HOME/.fuxi/events.db`。父目录不在则建；文件不在则由 store 自建。
///
/// 跟 `memory_cmd::resolve_db_path` 故意**不一样**——profile 钉死走 events.db，
/// 让仓颉/玄女在同一份 SQLite 里看到 user_profile + events，不必维持双库同步。
pub(crate) fn resolve_db_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    let home = std::env::var("HOME").context("无法解析 $HOME，请显式 --db 指定 SQLite 路径")?;
    let dir = PathBuf::from(&home).join(".fuxi");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    }
    Ok(dir.join("events.db"))
}

async fn run_set(db: &PathBuf, args: SetArgs) -> Result<()> {
    validate_key(&args.key)?;
    if args.value.trim().is_empty() {
        anyhow::bail!("value 不能为空");
    }
    let store = UserProfileStore::connect_file(db).await?;
    let id = store
        .record(NewProfile::new(args.key.clone(), args.value.clone()).with_source(args.source))
        .await?;
    println!(
        "{}",
        serde_json::json!({"id": id.to_string(), "key": args.key, "value": args.value})
    );
    Ok(())
}

async fn run_get(db: &PathBuf, args: GetArgs) -> Result<()> {
    validate_key(&args.key)?;
    let store = UserProfileStore::connect_file(db).await?;
    match store.get(&args.key).await? {
        Some(entry) => {
            println!("{}", serde_json::to_string(&entry)?);
            Ok(())
        }
        None => {
            // 不挂；玄女判断 stdout 是否空字符串。
            println!();
            Ok(())
        }
    }
}

async fn run_list(db: &PathBuf) -> Result<()> {
    let store = UserProfileStore::connect_file(db).await?;
    let rows = store.list_active().await?;
    if rows.is_empty() {
        // 友好提示走 stderr 不污染 JSON 通道；stdout 输出 `[]` 让 JSON parse 不挂。
        eprintln!("还没记任何用户画像。");
        println!("[]");
        return Ok(());
    }
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}

async fn run_unset(db: &PathBuf, args: UnsetArgs) -> Result<()> {
    validate_key(&args.key)?;
    let store = UserProfileStore::connect_file(db).await?;
    let Some(entry) = store.get(&args.key).await? else {
        eprintln!("key {:?} 没有活行——unset noop。", args.key);
        return Ok(());
    };
    // expire = 纯软删（valid_until=now，不插新行）。supersede 是 replace 语义，
    // 不适合 unset。alpha 在 user_profile.rs 加了 expire helper 直接用。
    store.expire(entry.id).await?;
    println!(
        "{}",
        serde_json::json!({"unset": args.key, "id": entry.id.to_string()})
    );
    Ok(())
}

/// CLI 层 key 校验：禁空 / 禁包含空格 / 禁带换行 tab——shell 拆参数前后值。
fn validate_key(key: &str) -> Result<()> {
    if key.trim().is_empty() {
        anyhow::bail!("key 不能为空");
    }
    if key.chars().any(|c| c.is_whitespace()) {
        anyhow::bail!("key 不能含空白字符（含空格/换行/tab）；多 token 用下划线");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_rejects_empty() {
        assert!(validate_key("").is_err());
        assert!(validate_key("   ").is_err());
    }

    #[test]
    fn validate_key_rejects_whitespace() {
        assert!(validate_key("user identity").is_err());
        assert!(validate_key("user\tidentity").is_err());
        assert!(validate_key("u\nv").is_err());
    }

    #[test]
    fn validate_key_accepts_underscore_and_dash() {
        assert!(validate_key("identity").is_ok());
        assert!(validate_key("tech_stack").is_ok());
        assert!(validate_key("project-erp").is_ok());
        assert!(validate_key("用户身份").is_ok());
    }

    /// 集成测——`:memory:` SQLite 走完整套路（先在文件层注入再走 connect_file
    /// 也行，但 connect_memory 更快+ 隔离）。
    ///
    /// CLI 的 run_* 只接 file path；测试这里直接调 store 验语义，不重测 store
    /// 已经测过的 record/get/list_active。
    #[tokio::test]
    async fn store_set_get_list_via_record_helper() {
        let store = UserProfileStore::connect_memory().await.unwrap();
        store
            .record(NewProfile::new("identity", "以琳，工程师").with_source("xuannv-explicit"))
            .await
            .unwrap();
        let got = store.get("identity").await.unwrap().unwrap();
        assert_eq!(got.value, "以琳，工程师");
        assert_eq!(got.source, "xuannv-explicit");

        let rows = store.list_active().await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    /// unset 走 `UserProfileStore::expire`——活行 valid_until=now，不插新行。
    /// list_active 返 0；老 id 不再 active。
    #[tokio::test]
    async fn unset_expires_active_entry_without_new_row() {
        let store = UserProfileStore::connect_memory().await.unwrap();
        let id = store
            .record(NewProfile::new("identity", "v1"))
            .await
            .unwrap();

        store.expire(id).await.unwrap();

        // get 看不到任何活行
        assert!(store.get("identity").await.unwrap().is_none());
        // list_active 也是空的
        assert!(store.list_active().await.unwrap().is_empty());
    }

    /// 二次 unset noop——CLI 层先 get 拿活行，没活行就 eprintln + Ok 退出，
    /// 不会调 expire 撞 NotFound。
    #[tokio::test]
    async fn unset_after_unset_finds_no_active() {
        let store = UserProfileStore::connect_memory().await.unwrap();
        let id = store
            .record(NewProfile::new("identity", "v1"))
            .await
            .unwrap();
        store.expire(id).await.unwrap();
        // CLI 路径会先 get：
        assert!(store.get("identity").await.unwrap().is_none());
    }
}
