"""thesis-v3 共用 bench 数据解析。

数据源：deliverables/thesis-v2/benchmarks/v2-2026-05-07.md
（v3 没独立存数据；v2 报告是当前实验真相源）

代码是第一真相源——这里的 parse 只对 markdown 表，
任何函数名/字段名变更需对照 v2-2026-05-07.md。
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT.parent / "thesis-v2" / "benchmarks" / "v2-2026-05-07.md"


def _read_report() -> str:
    return REPORT.read_text()


def parse_scalability():
    text = _read_report()
    m = re.search(
        r"##\s*3\.\s*Scalability[\s\S]*?\| worker_n [^\n]*\n((?:\|[^\n]*\n)+)",
        text,
    )
    if not m:
        raise RuntimeError("scalability 段未找到")
    rows = []
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("worker_n", "---", ":---:"):
            continue
        try:
            worker_n = int(cols[0])
            tps = float(cols[3].replace(",", ""))
            theoretical = float(cols[4].replace(",", ""))
            efficiency = float(cols[5].replace("%", ""))
        except (ValueError, IndexError):
            continue
        rows.append((worker_n, tps, theoretical, efficiency))
    return sorted(rows)


def parse_poll_scan():
    text = _read_report()
    m = re.search(
        r"##\s*2\.\s*Poll_ms[\s\S]*?\| poll_ms [^\n]*\n((?:\|[^\n]*\n)+)",
        text,
    )
    if not m:
        raise RuntimeError("poll_ms 段未找到")
    rows = []
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("poll_ms", "---"):
            continue
        try:
            poll_ms = int(cols[0])
            tps = float(cols[2].replace(",", ""))
            loss_pct = float(cols[4].replace("%", ""))
        except (ValueError, IndexError):
            continue
        rows.append((poll_ms, tps, loss_pct))
    return sorted(rows)


def parse_bus_stress():
    text = _read_report()
    m = re.search(
        r"##\s*4\.\s*事件总线[\s\S]*?\| subscribers [^\n]*\n((?:\|[^\n]*\n)+)",
        text,
    )
    if not m:
        raise RuntimeError("bus stress 段未找到")
    rows = []
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("subscribers", "---"):
            continue
        try:
            subs = int(cols[0])
            rate = int(cols[1].replace(",", ""))
            payload = cols[2]
            tps = float(cols[3].replace(",", ""))
            p50 = float(cols[4].replace(",", ""))
            p99 = float(cols[5].replace(",", ""))
            drops = int(cols[6].replace(",", ""))
        except (ValueError, IndexError):
            continue
        rows.append((subs, rate, payload, tps, p50, p99, drops))
    return rows


def parse_latency_quantiles():
    """读 baseline latency 表的 p50/p99 等。给 ch 5 文字引用用，eCDF 直接读 csv。"""
    text = _read_report()
    m = re.search(
        r"##\s*1\.[\s\S]*?\| metric [^\n]*\n((?:\|[^\n]*\n)+)",
        text,
    )
    if not m:
        raise RuntimeError("latency 段未找到")
    out = {}
    for line in m.group(1).strip().splitlines():
        cols = [c.strip() for c in line.strip("|").split("|")]
        if cols[0] in ("metric", "---"):
            continue
        try:
            out[cols[0]] = {
                "n": int(cols[1]),
                "min_ms": float(cols[2]),
                "p50_ms": float(cols[3]),
                "p99_ms": float(cols[4]),
                "max_ms": float(cols[5]),
            }
        except (ValueError, IndexError):
            continue
    return out
