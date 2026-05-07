"""图 5.2 poll_ms 参数消融——折线图 + 误差不需要因为 5-run median 已稳。"""
import re
from pathlib import Path
import matplotlib.pyplot as plt
from style import apply, DISCRETE_PALETTE

apply()

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "v2-2026-05-07.md"
OUT = Path(__file__).parent / "fig-5-2-poll-scan.png"


def parse_poll_scan(md_text: str):
    """从 markdown 抓 poll_scan 表的 (poll_ms, tps, overhead) 列。"""
    m = re.search(
        r"##\s*2\.\s*Poll_ms.*?\| poll_ms .*?\n((?:\|.*\n)+)",
        md_text,
        re.DOTALL,
    )
    if not m:
        raise RuntimeError("poll_scan 表未找到")
    rows = []
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("poll_ms", "---", ":---:"):
            continue
        try:
            poll_ms = int(cols[0])
            tps = float(cols[2].replace(",", ""))
            overhead = float(cols[4].replace("%", ""))
        except (ValueError, IndexError):
            continue
        rows.append((poll_ms, tps, overhead))
    return sorted(rows)


def main():
    rows = parse_poll_scan(REPORT.read_text())
    polls = [r[0] for r in rows]
    tps = [r[1] for r in rows]
    overhead = [r[2] for r in rows]

    fig, ax1 = plt.subplots()
    ax1.plot(polls, tps, "o-", color=DISCRETE_PALETTE[0], label="吞吐 (tasks/s)")
    ax1.set_xlabel("poll_ms (ms)")
    ax1.set_ylabel("吞吐 (tasks/s)", color=DISCRETE_PALETTE[0])
    ax1.tick_params(axis="y", labelcolor=DISCRETE_PALETTE[0])
    ax1.set_xscale("log")

    ax2 = ax1.twinx()
    ax2.plot(polls, overhead, "s--", color=DISCRETE_PALETTE[3], label="调度损耗 η (%)")
    ax2.set_ylabel("调度损耗 η (%)", color=DISCRETE_PALETTE[3])
    ax2.tick_params(axis="y", labelcolor=DISCRETE_PALETTE[3])
    ax2.grid(False)

    lines1, labels1 = ax1.get_legend_handles_labels()
    lines2, labels2 = ax2.get_legend_handles_labels()
    ax1.legend(lines1 + lines2, labels1 + labels2, loc="center right")

    fig.tight_layout()
    fig.savefig(OUT)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
