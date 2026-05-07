"""图 5.4 任务派发延迟分布——min/p50/p99/max 柱状图（5-run median）。

数据来源是 baseline 的 latency 段：
| metric | sample_n | min_ms | p50_ms | p99_ms | max_ms |
| task_dispatch | ... | ... | ... | ... | ... |
"""
import re
from pathlib import Path
import matplotlib.pyplot as plt
from style import apply, DISCRETE_PALETTE

apply()

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "v2-2026-05-07.md"
OUT = Path(__file__).parent / "fig-5-4-dispatch-latency.png"


def parse_latency(md_text: str):
    """找 baseline 段里的 latency 表，返回 task_dispatch 行的 (min, p50, p99, max)。"""
    m = re.search(
        r"##\s*1\.\s*Baseline[\s\S]*?\| metric [^\n]*\n((?:\|[^\n]*\n)+)",
        md_text,
    )
    if not m:
        raise RuntimeError("baseline latency 段未找到")
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] != "task_dispatch":
            continue
        return (
            float(cols[2].replace(",", "")),  # min
            float(cols[3].replace(",", "")),  # p50
            float(cols[4].replace(",", "")),  # p99
            float(cols[5].replace(",", "")),  # max
        )
    raise RuntimeError("task_dispatch 行未找到")


def main():
    quantiles = ["min", "p50", "p99", "max"]
    values = parse_latency(REPORT.read_text())

    fig, ax = plt.subplots()
    bars = ax.bar(quantiles, values, color=DISCRETE_PALETTE[:4], edgecolor="black", linewidth=0.6)
    ax.set_ylabel("延迟 (ms)")
    ax.set_title("任务派发延迟分布 (n=500, 5-run)")
    for b, v in zip(bars, values):
        ax.text(b.get_x() + b.get_width() / 2, v, f"{v:.2f}", ha="center", va="bottom", fontsize=9)
    fig.tight_layout()
    fig.savefig(OUT)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
