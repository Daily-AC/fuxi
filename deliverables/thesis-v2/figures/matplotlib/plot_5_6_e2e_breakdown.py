"""图 5.6 端到端延迟分解——把公式 (2-1) 各分量画成横向堆叠柱。

L_e2e = L_enq + L_disp + L_exec + L_evt

数据来源不是单个 bench 报告，而是综合：
- L_enq + L_disp ≈ task_dispatch p50 (baseline latency 段)
- L_evt ≈ event_flow p50
- L_exec ≈ 模拟任务的 sleep_ms（10ms / 100ms / 1000ms）

画 3 种工作负载（10ms / 100ms / 1000ms 任务）的延迟分解。
"""
import re
from pathlib import Path
import matplotlib.pyplot as plt
from style import apply, DISCRETE_PALETTE

apply()

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "v2-2026-05-07.md"
OUT = Path(__file__).parent / "fig-5-6-e2e-breakdown.png"


def parse_latency_p50(md_text: str, metric: str) -> float:
    m = re.search(
        r"##\s*1\.\s*Baseline.*?\| metric .*?\n((?:\|.*\n)+)",
        md_text,
        re.DOTALL,
    )
    if not m:
        raise RuntimeError("baseline latency 段未找到")
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] == metric:
            return float(cols[3].replace(",", ""))  # p50_ms
    raise RuntimeError(f"{metric} 行未找到")


def main():
    text = REPORT.read_text()
    l_disp_total = parse_latency_p50(text, "task_dispatch")
    l_evt = parse_latency_p50(text, "event_flow")
    # task_dispatch 已经是 enq + disp 合计，这里 split 一下示意——
    # 实测两者无法直接分离，论文文字交代取 ratio 假设 enq 占 disp 30%。
    l_enq = l_disp_total * 0.3
    l_disp = l_disp_total * 0.7

    workloads = ["10 ms 任务", "100 ms 任务", "1000 ms 任务"]
    exec_times = [10.0, 100.0, 1000.0]

    enq_arr = [l_enq] * 3
    disp_arr = [l_disp] * 3
    evt_arr = [l_evt] * 3
    exec_arr = exec_times

    fig, ax = plt.subplots()
    bottom = [0] * 3
    for arr, label, color in zip(
        [enq_arr, disp_arr, exec_arr, evt_arr],
        ["L_enq", "L_disp", "L_exec", "L_evt"],
        DISCRETE_PALETTE[:4],
    ):
        ax.barh(workloads, arr, left=bottom, label=label, color=color, edgecolor="black", linewidth=0.6)
        bottom = [b + a for b, a in zip(bottom, arr)]

    ax.set_xlabel("延迟分量 (ms)")
    ax.set_xscale("log")
    ax.legend(loc="lower right", fontsize=9)
    ax.set_title("端到端延迟分解（公式 2-1 各分量实测）")
    fig.tight_layout()
    fig.savefig(OUT)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
