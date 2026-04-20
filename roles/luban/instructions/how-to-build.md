# 工序：看 → 测 → 实 → 验

## 第一步 · 看（5 分钟内）

```
Read    最相关的 1-3 个文件
Grep    相关 trait / fn / 类型的所有引用
Bash    cargo check -p <crate>     # 确认起点能编译
```

不读完不动手。

## 第二步 · 写测试（RED）

新功能 / bug fix 都从**失败的测试**开始。

```rust
#[test]
fn quick_sort_handles_empty() {
    let mut v: Vec<i32> = vec![];
    quick_sort(&mut v);
    assert!(v.is_empty());
}
```

跑：

```
Bash    cargo test -p <crate> --no-fail-fast    # 必须看到 RED
```

如果测试**直接通过**——说明你在测既有行为，不是新行为。重写。

## 第三步 · 写实装（GREEN）

写**最小**让测试绿的代码。

- 不顺手做无关重构。
- 不为"未来需求"加额外开关 / 配置。
- 不写 plan 注释 / TODO。

跑：

```
Bash    cargo test -p <crate>     # 必须 GREEN
```

## 第四步 · 跑全门禁（VERIFY）

```
Bash    cargo fmt --all --check
Bash    cargo clippy -p <crate> --all-targets -- -D warnings
Bash    cargo test -p <crate>
```

任一红——回到第三步修。**不动测试去迁就实装**。

## 第五步 · 汇报

简短一两段：
- 改了哪些文件
- 测试结果（X passed）
- 是否需要授权（commit / push）

## 紧急情况：测试无解释失败

按 `superpowers:systematic-debugging` 的工序：
1. 复现：精确指令 + 期望 vs 实际
2. 隔离：缩到最小 case
3. 定位：bisect 输入 / git bisect commit
4. 假设：写下 ≥2 个可能根因
5. 测：每条假设最便宜的实验

不要乱改一通。
