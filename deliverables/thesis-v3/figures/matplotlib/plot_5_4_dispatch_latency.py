"""图 5-4 v3：任务派发延迟 eCDF（取代 v2 min/p50/p99/max 柱状图）。

按 figure-design-conventions.md §2.3：bar plot 表分布是反公理，改成 eCDF。
- 数据源：deliverables/thesis-v3/benchmarks/latency-samples.csv（task_dispatch 列）
- x ms log scale；y cumulative fraction 0-1
- p50 / p99 用水平虚线 annotate
- 单色深蓝（OKABE_ITO[0]），单根线
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
OUT = Path(__file__).parent / "fig-5-4-dispatch-latency.pdf"
METRIC = "task_dispatch"


def load_samples(metric: str) -> np.ndarray:
    if not CSV.exists():
        raise FileNotFoundError(
            f"latency-samples.csv 不存在：{CSV}\n"
            f"先跑 cargo bench -p fuxi-cli --bench run_baseline 让 dump_samples_csv 落盘。"
        )
    xs = []
    with CSV.open() as f:
        for row in csv.DictReader(f):
            if row["metric"] == metric:
                xs.append(float(row["sample_us"]) / 1000.0)  # us → ms
    if not xs:
        raise RuntimeError(f"csv 中无 metric={metric} 行")
    return np.sort(np.array(xs))


def ecdf_xy(samples: np.ndarray):
    n = len(samples)
    y = np.arange(1, n + 1) / n
    return samples, y


def main():
    samples = load_samples(METRIC)
    xs, ys = ecdf_xy(samples)

    fig, ax = plt.subplots(figsize=(SINGLE, SINGLE / GOLDEN))
    ax.plot(xs, ys, color=OKABE_ITO[0], linewidth=1.4, zorder=3)
    ax.set_xscale("log")
    ax.set_xlabel("任务派发延迟 (ms, log)")
    ax.set_ylabel("Cumulative fraction")
    ax.set_ylim(0, 1.02)
    ax.grid(True, axis="y", alpha=0.25)

    p50 = float(np.percentile(samples, 50))
    p99 = float(np.percentile(samples, 99))
    for q, val, label in [(0.5, p50, "p50"), (0.99, p99, "p99")]:
        ax.axhline(q, ls="--", color="0.55", linewidth=0.6)
        ax.axvline(val, ls="--", color="0.55", linewidth=0.6)
        ax.annotate(f"{label} = {val:.1f} ms", xy=(val, q),
                    xytext=(4, -10), textcoords="offset points", fontsize=7)

    ax.text(0.04, 0.95, f"n = {len(samples)}", transform=ax.transAxes,
            fontsize=7, color="0.35", verticalalignment="top")

    fig.tight_layout()
    fig.savefig(OUT)
    fig.savefig(OUT.with_suffix(".png"), dpi=200)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
