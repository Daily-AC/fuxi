"""图 5-1 v3：吞吐与 scaling efficiency 双子图（删 dual y-axis）。

按 figure-design-conventions.md §2.2：
- subplots(1,2)：左 throughput 含理论上限对照，右 efficiency 单独
- x 轴 log2 base，显式 ticks 1/2/4/8/16
- 自家深蓝实线 + 圆点；理论上限浅灰细虚线（不抢主角）
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from style import apply, OKABE_ITO, DOUBLE, GOLDEN  # noqa: E402

import matplotlib.pyplot as plt  # noqa: E402

sys.path.insert(0, str(Path(__file__).parent))
from _data import parse_scalability  # noqa: E402

apply()

OUT = Path(__file__).parent / "fig-5-1-scalability.pdf"


def main():
    rows = parse_scalability()
    workers = [r[0] for r in rows]
    tps = [r[1] for r in rows]
    theo = [r[2] for r in rows]
    eff = [r[3] for r in rows]

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(DOUBLE, DOUBLE / GOLDEN / 1.4))

    # ── 左：throughput vs N ──
    ax1.plot(workers, tps, "-o", color=OKABE_ITO[0], label="实测 fuxi", markersize=5, linewidth=1.6, zorder=3)
    ax1.plot(workers, theo, "--", color="0.55", label="理论上限", linewidth=0.9, zorder=2)
    ax1.set_xscale("log", base=2)
    ax1.set_xticks(workers)
    ax1.set_xticklabels([str(w) for w in workers])
    ax1.set_xlabel("Worker 数")
    ax1.set_ylabel("Tasks / s（5-run median）")
    ax1.legend(loc="upper left")
    ax1.grid(True, axis="y", alpha=0.25)

    # ── 右：scaling efficiency ──
    ax2.plot(workers, eff, "-s", color=OKABE_ITO[0], markersize=5, linewidth=1.6, zorder=3)
    ax2.axhline(100, ls="--", color="0.55", linewidth=0.9, zorder=2)
    ax2.set_xscale("log", base=2)
    ax2.set_xticks(workers)
    ax2.set_xticklabels([str(w) for w in workers])
    ax2.set_xlabel("Worker 数")
    ax2.set_ylabel("Scaling efficiency (%)")
    ax2.set_ylim(70, 105)
    ax2.grid(True, axis="y", alpha=0.25)

    fig.tight_layout()
    fig.savefig(OUT)
    fig.savefig(OUT.with_suffix(".png"), dpi=200)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
