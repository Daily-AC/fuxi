//! 斜杠命令注册表——把 REPL 里硬编在 `handle_key` 的 `if slash == "/help" ...`
//! 连串 if/else 迁移到数据驱动。
//
// 设计背景：
// - R7 要的是"抽象"而非"接线"。现在 repl.rs 2900+ 行里命令细节散落在键位处理、
//   输入 submit、回调多处——想上 Slash 浮层（R8）或 /help 自动生成（R11）
//   之前，得先把"命令"这个概念从 handler 里剥出来。
// - 社区成熟做法（zellij keybinds / helix commands / tokio console cmds）都是把
//   "显示名/快捷键/描述/执行动作"组成一行数据结构存一张表，入口代码只做 dispatch。
//   这里照同一套路，但把真正执行逻辑延到 γ 第二波再接——R7 只交付**数据层**。
// - `CommandAction` 选枚举不选 `Box<dyn Fn>`：
//   1. γ 第二波才接 ReplApp，现在没有成熟 handler 签名可 Box。
//   2. 枚举可 `Clone + Debug + PartialEq`，测试好写、浮层渲染好用。
//   3. `Theme(Option<String>)` 这种带参命令用枚举变体天然表达，Fn 要自己 parse。
// - `fuzzy_filter` 只做"前缀不分大小写匹配"，不做编辑距离。R8 浮层过滤用 —— 用户
//   键了 `/h` 应立刻筛出 `/help`，不希望 `/status` 因为一个 `h` 也上榜。
//   需要更聪明的匹配（编辑距离 / 子串 / 首字母）再说——YAGNI。
//
// WHY `allow(dead_code)`：R7 只落数据层，`register_default` / 三个 find 接口 γ
// 第二波（R11 /help 自动生成、R15 /theme 运行时切、R8 Slash 浮层）才有人 use。
// 移除 allow 的时机 = 有调用方 use 起来后。

#![allow(dead_code)]

/// 命令属于哪一组——/help 渲染分段、Slash 浮层分组用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    /// 导航类：/help /clear /quit 之类，跟业务无关的 REPL 元操作。
    Navigation,
    /// 外观类：/theme 等视觉切换。
    Appearance,
    /// 门客类：/kill /status 之类对 agent 军团的操作。
    Agent,
}

impl CommandCategory {
    /// /help 输出里的小标题。显示用，不要拿去做 key。
    pub fn label(self) -> &'static str {
        match self {
            CommandCategory::Navigation => "导航",
            CommandCategory::Appearance => "外观",
            CommandCategory::Agent => "门客",
        }
    }
}

/// 按 Enter 后 REPL 要执行什么——γ 第二波接到 ReplApp 的 handler。
///
/// WHY 不用 `Box<dyn Fn>`：见模块注释。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Help,
    Clear,
    Quit,
    Tree,
    /// `/theme` 无参时展示主题列表 / 循环切换；带名字则直接切过去。
    /// 具体语义由 γ 第二波决定，这里只传原始 `Option<String>`。
    Theme(Option<String>),
    Kill,
    Status,
    /// `/nodes` 打开远端 worker 拓扑 overlay（P6）。F6 等价快捷键。
    Nodes,
}

/// 一条命令的全部元数据。
///
/// - `slash`：含前导 `/` 的展示名，如 `"/help"`。作 key，全小写。
/// - `keybind`：可选全局快捷键（如 `Ctrl+L` 清屏），`None` 表示仅 slash 可触发。
///   格式保持人类可读字符串（`"Ctrl+L"` / `"Esc Esc"`），γ 第二波再解析成 crossterm
///   `KeyEvent`——registry 不耦合具体后端。
/// - `description`：一句话说明，/help 和浮层都显示。
/// - `category`：分组。
/// - `action`：触发时 REPL 要做的事（枚举）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub slash: &'static str,
    pub keybind: Option<&'static str>,
    pub description: &'static str,
    /// 参数名列表。空 = 无参命令；非空 = 至少需要用户补参数。
    pub arg_names: Vec<String>,
    pub category: CommandCategory,
    pub action: CommandAction,
}

/// 命令注册表。
///
/// 目前用 `Vec` 而不是 `HashMap`：命令数量是常量级（< 30），顺序还要保留（/help
/// 按注册顺序渲染），线性扫比 hash 开销更小也更可预测。
#[derive(Debug, Clone, Default)]
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// 注册一条命令。重复 slash **覆盖**旧条目——R15 /theme 运行时想替换自定义命令
    /// 时不需要 unregister 两步。
    pub fn register(&mut self, cmd: Command) {
        if let Some(slot) = self
            .commands
            .iter_mut()
            .find(|c| c.slash.eq_ignore_ascii_case(cmd.slash))
        {
            *slot = cmd;
        } else {
            self.commands.push(cmd);
        }
    }

    /// 全部命令，按注册顺序——/help 渲染要稳定次序。
    pub fn all(&self) -> &[Command] {
        &self.commands
    }

    /// 精确 slash 查找，大小写不敏感（`/HELP` == `/help`）。
    pub fn find_by_slash(&self, name: &str) -> Option<&Command> {
        self.commands
            .iter()
            .find(|c| c.slash.eq_ignore_ascii_case(name))
    }

    /// 快捷键查找，大小写不敏感（`ctrl+l` == `Ctrl+L`）。
    ///
    /// 多条命令绑同一个 keybind 时只返第一个——注册顺序决定优先级。
    pub fn find_by_keybind(&self, key: &str) -> Option<&Command> {
        self.commands
            .iter()
            .find(|c| matches!(c.keybind, Some(k) if k.eq_ignore_ascii_case(key)))
    }

    /// 前缀模糊过滤——Slash 浮层用。
    ///
    /// 契约：
    /// - 匹配 slash 名的**前缀**，不分大小写（`/h` 命中 `/help` 但不命中 `/status`）。
    /// - 空串返回全部——浮层刚弹出时显示所有命令。
    /// - 保持注册顺序。
    pub fn fuzzy_filter(&self, prefix: &str) -> Vec<&Command> {
        if prefix.is_empty() {
            return self.commands.iter().collect();
        }
        let lower = prefix.to_ascii_lowercase();
        self.commands
            .iter()
            .filter(|c| c.slash.to_ascii_lowercase().starts_with(&lower))
            .collect()
    }

    /// 生成 `/help` 输出：按 category 分组，每组内列 slash / keybind / description。
    /// 末尾追加一段键盘操作总览。
    ///
    /// WHY markdown 风格而非 ANSI：交给宿主用 `DialogueLine::System` 一整块推进
    /// 对话滚屏——宿主不 parse markdown，但 `**` / `#` / 列表符号让人眼读起来
    /// 结构清晰；未来换成 Paragraph + Line 渲染也能最低成本迁移。
    pub fn render_help_markdown(&self) -> String {
        let mut out = String::from("# 伏羲命令\n");

        // 按 Category 顺序聚——Navigation → Appearance → Agent。
        for cat in [
            CommandCategory::Navigation,
            CommandCategory::Appearance,
            CommandCategory::Agent,
        ] {
            let items: Vec<&Command> = self.commands.iter().filter(|c| c.category == cat).collect();
            if items.is_empty() {
                continue;
            }
            out.push_str(&format!("\n## {}\n", cat.label()));
            for cmd in items {
                match cmd.keybind {
                    Some(k) => {
                        out.push_str(&format!(
                            "- **{}** ({}) — {}\n",
                            cmd.slash, k, cmd.description
                        ));
                    }
                    None => {
                        out.push_str(&format!("- **{}** — {}\n", cmd.slash, cmd.description));
                    }
                }
            }
        }

        out.push_str("\n## 键盘\n");
        // 这段固定——跟 repl.rs 底部 hint 一致；改键位时同步改此处。
        for tip in [
            "文本选择 · 直接拖拽 · Cmd+C 复制 · Cmd+V 粘贴",
            "Tab 切 active · Shift+Tab 反向",
            "↑ / ↓ 翻 prompt 历史（输入框空时）",
            "Esc 双击中断 active · Ctrl+C 退出",
            "F2 事件流 · F4 任务列表 · F5 元信息",
        ] {
            out.push_str(&format!("- {tip}\n"));
        }
        out
    }
}

/// 默认命令集——γ 第二波接到 main.rs 初始化。
///
/// 命令顺序决定 /help 渲染顺序，改动注意保持"导航 → 外观 → 门客"分组连续。
pub fn register_default() -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    reg.register(Command {
        slash: "/help",
        keybind: None,
        description: "列出全部命令",
        arg_names: vec![],
        category: CommandCategory::Navigation,
        action: CommandAction::Help,
    });
    reg.register(Command {
        slash: "/clear",
        keybind: Some("Ctrl+L"),
        description: "清空滚屏",
        arg_names: vec![],
        category: CommandCategory::Navigation,
        action: CommandAction::Clear,
    });
    reg.register(Command {
        slash: "/quit",
        keybind: None,
        description: "退出 REPL",
        arg_names: vec![],
        category: CommandCategory::Navigation,
        action: CommandAction::Quit,
    });
    reg.register(Command {
        slash: "/theme",
        keybind: None,
        description: "切换主题（可带名字，不带则循环）",
        arg_names: vec!["name".to_string()],
        category: CommandCategory::Appearance,
        action: CommandAction::Theme(None),
    });
    reg.register(Command {
        slash: "/tree",
        keybind: None,
        description: "配置左侧任务树（on/off/toggle）",
        arg_names: vec!["on|off|toggle".to_string()],
        category: CommandCategory::Navigation,
        action: CommandAction::Tree,
    });
    reg.register(Command {
        slash: "/kill",
        keybind: None,
        description: "关掉指定门客",
        arg_names: vec![],
        category: CommandCategory::Agent,
        action: CommandAction::Kill,
    });
    reg.register(Command {
        slash: "/status",
        keybind: None,
        description: "展示门客状态",
        arg_names: vec![],
        category: CommandCategory::Agent,
        action: CommandAction::Status,
    });
    reg.register(Command {
        slash: "/nodes",
        keybind: Some("F6"),
        description: "查看远端 worker 拓扑（alive/stale/inflight）",
        arg_names: vec![],
        category: CommandCategory::Agent,
        action: CommandAction::Nodes,
    });
    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    // —— 契约三件套（team-lead 指定）——

    #[test]
    fn registry_find_by_slash_works() {
        let reg = register_default();
        let help = reg
            .find_by_slash("/help")
            .expect("默认 registry 必含 /help");
        assert_eq!(help.slash, "/help");
        assert_eq!(help.action, CommandAction::Help);

        // 大小写不敏感。
        assert!(reg.find_by_slash("/HELP").is_some());

        // 不存在的命令返回 None。
        assert!(reg.find_by_slash("/nosuch").is_none());
    }

    #[test]
    fn fuzzy_filter_respects_prefix() {
        let reg = register_default();

        // `/h` 只命中 `/help`——不能把 `/status` 之类误筛进来。
        let by_h = reg.fuzzy_filter("/h");
        assert_eq!(by_h.len(), 1);
        assert_eq!(by_h[0].slash, "/help");

        // `/` 前缀命中全部。
        let by_slash = reg.fuzzy_filter("/");
        assert_eq!(by_slash.len(), reg.all().len());

        // 空串也命中全部。
        assert_eq!(reg.fuzzy_filter("").len(), reg.all().len());

        // 大小写不敏感。
        let by_upper = reg.fuzzy_filter("/HE");
        assert_eq!(by_upper.len(), 1);
        assert_eq!(by_upper[0].slash, "/help");

        // 前缀不是子串：`help` 不带 `/` 不命中。
        assert!(reg.fuzzy_filter("help").is_empty());
    }

    #[test]
    fn every_default_command_has_description_and_category() {
        let reg = register_default();
        assert!(!reg.all().is_empty(), "默认 registry 不能是空的");

        for cmd in reg.all() {
            assert!(cmd.slash.starts_with('/'), "命令 {} 缺 / 前缀", cmd.slash);
            assert!(
                !cmd.description.trim().is_empty(),
                "命令 {} 缺 description",
                cmd.slash
            );
            // category 是枚举，存在性靠类型保证；这里校验 label 非空即可——
            // 防止将来新加分类忘填 `label()`。
            assert!(
                !cmd.category.label().is_empty(),
                "命令 {} 的 category 无 label",
                cmd.slash
            );
        }
    }

    // —— 其他不变量 ——

    #[test]
    fn find_by_keybind_works() {
        let reg = register_default();
        let clear = reg.find_by_keybind("Ctrl+L").expect("/clear 绑了 Ctrl+L");
        assert_eq!(clear.slash, "/clear");

        // 大小写不敏感。
        assert!(reg.find_by_keybind("ctrl+l").is_some());

        // 未绑快捷键的查询返 None。
        assert!(reg.find_by_keybind("F12").is_none());
    }

    #[test]
    fn register_overrides_same_slash() {
        let mut reg = register_default();
        let before = reg.all().len();

        reg.register(Command {
            slash: "/help",
            keybind: Some("F1"),
            description: "改过的 help",
            arg_names: vec![],
            category: CommandCategory::Navigation,
            action: CommandAction::Help,
        });

        // 总数不变——原地覆盖而非追加。
        assert_eq!(reg.all().len(), before);
        let help = reg.find_by_slash("/help").unwrap();
        assert_eq!(help.keybind, Some("F1"));
        assert_eq!(help.description, "改过的 help");
    }

    #[test]
    fn default_registry_covers_required_commands() {
        let reg = register_default();
        for slash in [
            "/help", "/clear", "/quit", "/theme", "/tree", "/kill", "/status",
        ] {
            assert!(
                reg.find_by_slash(slash).is_some(),
                "默认 registry 缺 {}",
                slash
            );
        }
    }

    #[test]
    fn theme_command_carries_optional_arg() {
        // /theme 默认无参——γ 第二波 parse 用户输入再构 Theme(Some(...))。
        let reg = register_default();
        let theme = reg.find_by_slash("/theme").unwrap();
        assert_eq!(theme.action, CommandAction::Theme(None));
    }

    #[test]
    fn theme_command_exposes_arg_names_metadata() {
        let reg = register_default();
        let theme = reg.find_by_slash("/theme").expect("应有 /theme");
        assert_eq!(theme.arg_names, vec!["name".to_string()]);
    }

    // ───────── R11 /help 自动生成 ─────────

    #[test]
    fn help_output_lists_all_commands() {
        let reg = register_default();
        let text = reg.render_help_markdown();
        for cmd in reg.all() {
            assert!(
                text.contains(cmd.slash),
                "help 输出应含 {}：\n{}",
                cmd.slash,
                text
            );
            assert!(
                text.contains(cmd.description),
                "help 输出应含 {} 的 description：\n{}",
                cmd.slash,
                text
            );
        }
    }

    #[test]
    fn help_groups_by_category() {
        let reg = register_default();
        let text = reg.render_help_markdown();
        // 必须出现三组标题。
        for label in ["导航", "外观", "门客"] {
            assert!(text.contains(label), "help 应有分组标题 {label}：\n{text}");
        }
        // 顺序：导航 → 外观 → 门客。任何一组前后位置错都应红灯。
        let nav = text.find("导航").expect("导航 标题必在");
        let app = text.find("外观").expect("外观 标题必在");
        let agent = text.find("门客").expect("门客 标题必在");
        assert!(nav < app && app < agent, "分组顺序应是 导航<外观<门客");
    }

    #[test]
    fn help_includes_keyboard_tips() {
        let reg = register_default();
        let text = reg.render_help_markdown();
        // "键盘" 段标题。
        assert!(text.contains("键盘"), "应有键盘段标题：\n{text}");
        // 关键 tips 抽检：
        for keyword in ["Cmd+C", "Tab", "Esc", "Ctrl+C", "F2"] {
            assert!(text.contains(keyword), "键盘 tips 应含 {keyword}：\n{text}");
        }
    }

    #[test]
    fn help_renders_keybind_when_present() {
        // /clear 绑了 Ctrl+L——help 行里要同时带 slash 和 keybind。
        let reg = register_default();
        let text = reg.render_help_markdown();
        // 捞到 /clear 行——包含 Ctrl+L 即合格。
        let clear_line = text
            .lines()
            .find(|l| l.contains("/clear"))
            .expect("应有 /clear 行");
        assert!(
            clear_line.contains("Ctrl+L"),
            "/clear 行应带 Ctrl+L：{clear_line}"
        );
    }
}
