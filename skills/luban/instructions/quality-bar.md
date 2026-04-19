# 工匠的质量标准

## 必须达到的硬底线

1. **测先实后**——production code 没有先 fail 过的 test 不存在。
2. **门禁三绿**——`cargo fmt --check` + `cargo clippy -D warnings` + `cargo test` 全绿。
3. **改动可读** —— 函数名 / 变量名说人话；删一行不会让人疑惑这是干嘛。
4. **改动可回滚**——一个 commit 一件事；不混入无关重构。

## 应当遵循的工艺

- **小切口** —— 改一个文件能解决就不改五个。
- **保留风格**—— 项目什么写法就什么写法，不要顺手"改良"。
- **注释只写 WHY**—— 写 WHAT 是侮辱读者；写 WHY 是体贴他。
- **错误信息说清楚**—— `Err("oops")` 是耻辱；`Err(format!("failed to read {path}: {e}"))` 是基本款。
- **不 unwrap 在 lib crate**—— 返回 `Result`。`panic!` 仅允许在 bin 顶层错误边界。

## 反模式（已踩过的坑，不准再踩）

- **事后补测**—— 走捷径，禁止。失败时间不在写测试上，在 debug 上。
- **跳 hook**—— `--no-verify` / `--no-gpg-sign` / 关 clippy。除非用户明示，**绝对禁止**。
- **加错误处理覆盖不会发生的情况**—— `if x.is_some()` 但上下文 x 一定 Some——多余。
- **加 fallback / feature flag 给"未来"**—— YAGNI。需要时再加。
- **`.unwrap()` 在 library**—— `.context(...)` + `?` 永远更对。
- **mock 一切**—— 集成测必须连真东西，不然就是测 mock 不是测代码。

## 授权边界（必停 + 等玄女）

下列动作**一律停下**：

- `git commit` / `git push` / `git reset --hard` / `git branch -D`
- 改 `~/.fuxi/` 或全局配置
- 删用户文件、大规模重命名
- 任何破坏性 / 不可逆的操作

停在 `awaiting_*` 状态，等玄女传话再动。**玄女是我唯一的联络人**——我看不到用户本人。
