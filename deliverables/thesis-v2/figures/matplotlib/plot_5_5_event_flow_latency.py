"""图 5.5 跨节点事件流延迟分布——baseline latency 表的 event_flow 行。"""
import re
from pathlib import Path
import matplotlib.pyplot as plt
from style import apply, DISCRETE_PALETTE

apply()

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "v2-2026-05-07.md"
OUT = Path(__file__).parent / "fig-5-5-event-flow-latency.png"


def parse_event_flow(md_text: str):
    m = re.search(
        r"##\s*1\.\s*Baseline.*?\| metric .*?\n((?:\|.*\n)+)",
        md_text,
        re.DOTALL,
    )
    if not m:
        raise RuntimeError("baseline latency 段未找到")
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] != "event_flow":
            continue
        return (
            float(cols[2].replace(",", "")),
            float(cols[3].replace(",", "")),
            float(cols[4].replace(",", "")),
            float(cols[5].replace(",", "")),
        )
    raise RuntimeError("event_flow 行未找到")


def main():
    quantiles = ["min", "p50", "p99", "max"]
    values = parse_event_flow(REPORT.read_text())

    fig, ax = plt.subplots()
    bars = ax.bar(quantiles, values, color=DISCRETE_PALETTE[:4], edgecolor="black", linewidth=0.6)
    ax.set_ylabel("延迟 (ms)")
    ax.set_title("跨节点事件流延迟分布 (n=500, 5-run)")
    for b, v in zip(bars, values):
        ax.text(b.get_x() + b.get_width() / 2, v, f"{v:.3f}", ha="center", va="bottom", fontsize=9)
    fig.tight_layout()
    fig.savefig(OUT)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
