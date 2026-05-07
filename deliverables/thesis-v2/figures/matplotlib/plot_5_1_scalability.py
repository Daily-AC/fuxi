"""图 5.1 吞吐与 worker 数线性扩展。
读 deliverables/thesis-v2/benchmarks/v2-2026-05-07.md 的 scalability 表，
画实测 vs 理想线性折线 + scaling efficiency。
"""
import re
from pathlib import Path
import matplotlib.pyplot as plt
from style import apply, DISCRETE_PALETTE

apply()

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "v2-2026-05-07.md"
OUT = Path(__file__).parent / "fig-5-1-scalability.png"


def parse_scalability(md_text: str):
    """从 markdown 抓 scalability 表的 (worker_n, tps, theoretical, efficiency) 列。"""
    # 找 "## 3. Scalability" 段
    m = re.search(
        r"##\s*3\.\s*Scalability[\s\S]*?\| worker_n [^\n]*\n((?:\|[^\n]*\n)+)",
        md_text,
    )
    if not m:
        raise RuntimeError("scalability 表未找到")
    rows = []
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("worker_n", "---", ":---:"):  # 表头/分隔符
            continue
        try:
            worker_n = int(cols[0])
            tps = float(cols[3].replace(",", ""))
            theoretical = float(cols[4].replace(",", ""))
            eff_str = cols[5].replace("%", "")
            efficiency = float(eff_str)
        except (ValueError, IndexError):
            continue
        rows.append((worker_n, tps, theoretical, efficiency))
    return sorted(rows)


def main():
    rows = parse_scalability(REPORT.read_text())
    workers = [r[0] for r in rows]
    tps = [r[1] for r in rows]
    ideal = [r[2] for r in rows]
    eff = [r[3] for r in rows]

    fig, ax1 = plt.subplots()
    ax1.plot(workers, tps, "o-", color=DISCRETE_PALETTE[0], label="实测吞吐 (tasks/s)")
    ax1.plot(workers, ideal, "k--", alpha=0.4, label="理论上限")
    ax1.set_xlabel("Worker 数")
    ax1.set_ylabel("吞吐 (tasks/s)", color=DISCRETE_PALETTE[0])
    ax1.tick_params(axis="y", labelcolor=DISCRETE_PALETTE[0])
    ax1.set_xticks(workers)

    ax2 = ax1.twinx()
    ax2.plot(workers, eff, "s--", color=DISCRETE_PALETTE[1], label="scaling efficiency")
    ax2.set_ylabel("Scaling Efficiency (%)", color=DISCRETE_PALETTE[1])
    ax2.tick_params(axis="y", labelcolor=DISCRETE_PALETTE[1])
    ax2.set_ylim(0, 105)
    ax2.grid(False)

    # 合并 legend
    lines1, labels1 = ax1.get_legend_handles_labels()
    lines2, labels2 = ax2.get_legend_handles_labels()
    ax1.legend(lines1 + lines2, labels1 + labels2, loc="lower right")

    fig.tight_layout()
    fig.savefig(OUT)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
