"""图 5.3 事件总线纯压测 publish 吞吐 vs subscriber 数 (按 payload 分面)。"""
import re
from collections import defaultdict
from pathlib import Path
import matplotlib.pyplot as plt
from style import apply, DISCRETE_PALETTE

apply()

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "v2-2026-05-07.md"
OUT = Path(__file__).parent / "fig-5-3-bus-stress.png"


def parse_bus_stress(md_text: str):
    """从 markdown 抓 bus_stress 表的所有 cell 数据。
    返回 dict[(n_sub, rate, payload)] = (publish_tps, p50_us, p99_us, drops)
    """
    m = re.search(
        r"##\s*4\.\s*事件总线.*?\| subscribers .*?\n((?:\|.*\n)+)",
        md_text,
        re.DOTALL,
    )
    if not m:
        raise RuntimeError("bus_stress 表未找到")
    out = {}
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("subscribers", "---", ":---:"):
            continue
        try:
            n_sub = int(cols[0])
            rate = int(cols[1])
            payload = cols[2]
            pub_tps = float(cols[3].replace(",", ""))
            p50_us = float(cols[4].replace(",", ""))
            p99_us = float(cols[5].replace(",", ""))
            drops = int(cols[6])
        except (ValueError, IndexError):
            continue
        out[(n_sub, rate, payload)] = (pub_tps, p50_us, p99_us, drops)
    return out


def main():
    data = parse_bus_stress(REPORT.read_text())
    n_subs = sorted({k[0] for k in data})
    rates = sorted({k[1] for k in data})
    payloads = ["small", "large"]

    fig, axes = plt.subplots(1, 2, figsize=(11, 4.0), sharey=True)
    for ax, payload in zip(axes, payloads):
        for i, rate in enumerate(rates):
            ys = []
            for n_sub in n_subs:
                v = data.get((n_sub, rate, payload))
                ys.append(v[0] if v else 0.0)
            ax.plot(
                n_subs,
                ys,
                "o-",
                color=DISCRETE_PALETTE[i % len(DISCRETE_PALETTE)],
                label=f"{rate:,} ev/s",
            )
        ax.set_title(f"payload = {payload}")
        ax.set_xlabel("Subscriber 数")
        ax.set_xticks(n_subs)
        ax.set_xscale("log")
        ax.legend(loc="lower left", fontsize=9)

    axes[0].set_ylabel("publish 吞吐 (events/s)")
    fig.tight_layout()
    fig.savefig(OUT)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
