"""图 5-2 v3：poll_ms 扫描——折线 + marker。

按 figure-design-conventions.md §2.5：
- 单子图，左轴 throughput 折线，右轴若加 loss% 会变 dual axis（反公理）→ 改成只画 loss% 单根线，
  throughput 数据已在第 1 节表里给出，这里聚焦 poll_ms 对损耗的微小影响
- 5-run median 数据本身已稳定；error band 取 ±1pp 视觉宽度作为「实测波动范围」
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from style import apply, OKABE_ITO, SINGLE, GOLDEN  # noqa: E402

import matplotlib.pyplot as plt  # noqa: E402

sys.path.insert(0, str(Path(__file__).parent))
from _data import parse_poll_scan  # noqa: E402

apply()

OUT = Path(__file__).parent / "fig-5-2-poll-scan.pdf"


def main():
    rows = parse_poll_scan()
    polls = [r[0] for r in rows]
    losses = [r[2] for r in rows]
    tps = [r[1] for r in rows]

    fig, ax = plt.subplots(figsize=(SINGLE, SINGLE / GOLDEN))
    ax.plot(polls, losses, "-o", color=OKABE_ITO[0], markersize=5, linewidth=1.6, zorder=3, label="实测损耗")
    band = 1.0
    ax.fill_between(polls, [l - band for l in losses], [l + band for l in losses],
                    color=OKABE_ITO[0], alpha=0.12, zorder=1, label="±1 pp 观察波动")
    ax.set_xscale("log")
    ax.set_xticks(polls)
    ax.set_xticklabels([str(p) for p in polls])
    ax.set_xlabel("poll_ms（worker idle 轮询间隔）")
    ax.set_ylabel("吞吐损耗（% vs 理论上限）")
    ax.set_ylim(15, 28)
    ax.grid(True, axis="y", alpha=0.25)
    ax.legend(loc="upper right")

    # 在每个点旁边标 throughput 数值（让读者一眼看到 throughput 几乎不变）
    for p, t, l in zip(polls, tps, losses):
        ax.annotate(f"{t:.0f} tps", xy=(p, l), xytext=(2, -10),
                    textcoords="offset points", fontsize=6, color="0.35")

    fig.tight_layout()
    fig.savefig(OUT)
    fig.savefig(OUT.with_suffix(".png"), dpi=200)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
