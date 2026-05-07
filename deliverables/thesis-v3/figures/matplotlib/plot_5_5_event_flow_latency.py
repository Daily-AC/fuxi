"""图 5-5 v3：跨节点事件流延迟 eCDF（取代 v2 bar）。

数据源同 5-4 但 metric=event_flow（亚毫秒分布）。
x μs log scale；其余风格同 5-4。
"""
import csv
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent.parent))
from style import apply, OKABE_ITO, SINGLE, GOLDEN  # noqa: E402

import matplotlib.pyplot as plt  # noqa: E402

apply()

ROOT = Path(__file__).resolve().parents[2]
CSV = ROOT / "benchmarks" / "latency-samples.csv"
OUT = Path(__file__).parent / "fig-5-5-event-flow-latency.pdf"
METRIC = "event_flow"


def load_samples(metric: str) -> np.ndarray:
    if not CSV.exists():
        raise FileNotFoundError(
            f"latency-samples.csv 不存在：{CSV}\n"
            f"先跑 cargo bench -p fuxi-cli --bench run_baseline。"
        )
    xs = []
    with CSV.open() as f:
        for row in csv.DictReader(f):
            if row["metric"] == metric:
                xs.append(float(row["sample_us"]))  # 保留 us 单位
    if not xs:
        raise RuntimeError(f"csv 中无 metric={metric} 行")
    return np.sort(np.array(xs))


def main():
    samples = load_samples(METRIC)
    n = len(samples)
    ys = np.arange(1, n + 1) / n

    fig, ax = plt.subplots(figsize=(SINGLE, SINGLE / GOLDEN))
    ax.plot(samples, ys, color=OKABE_ITO[0], linewidth=1.4, zorder=3)
    ax.set_xscale("log")
    ax.set_xlabel("跨节点事件流延迟 (μs, log)")
    ax.set_ylabel("Cumulative fraction")
    ax.set_ylim(0, 1.02)
    ax.grid(True, axis="y", alpha=0.25)

    p50 = float(np.percentile(samples, 50))
    p99 = float(np.percentile(samples, 99))
    for q, val, label in [(0.5, p50, "p50"), (0.99, p99, "p99")]:
        ax.axhline(q, ls="--", color="0.55", linewidth=0.6)
        ax.axvline(val, ls="--", color="0.55", linewidth=0.6)
        ax.annotate(f"{label} = {val:.1f} μs", xy=(val, q),
                    xytext=(4, -10), textcoords="offset points", fontsize=7)

    ax.text(0.04, 0.95, f"n = {n}", transform=ax.transAxes,
            fontsize=7, color="0.35", verticalalignment="top")

    fig.tight_layout()
    fig.savefig(OUT)
    fig.savefig(OUT.with_suffix(".png"), dpi=200)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
