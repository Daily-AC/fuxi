#!/usr/bin/env bash
set -euo pipefail

# 解析 /tmp/fuxi.log（或指定文件）中的桥接延迟与玄女 busy 入队指标。
# 用法：
#   scripts/bridge-lag-report.sh
#   scripts/bridge-lag-report.sh /tmp/fuxi.log

LOG_FILE="${1:-/tmp/fuxi.log}"

if [[ ! -f "$LOG_FILE" ]]; then
  echo "日志文件不存在: $LOG_FILE" >&2
  exit 2
fi

awk '
BEGIN {
  bridge_count = 0
  busy_enqueue_count = 0
  threshold_count = 0
  threshold_ms = 3000
}

/bridge: 转发门客(任务终态回报到玄女|下线回报到玄女)/ {
  bridge_count++
  if (index($0, "interrupt_first=true") > 0) {
    interrupt_true_count++
  }
  if (index($0, "interrupt_first=false") > 0) {
    interrupt_false_count++
  }
  if (match($0, /lag_ms=[0-9]+/)) {
    value = substr($0, RSTART + 7, RLENGTH - 7) + 0
    lag_sum += value
    if (bridge_count == 1 || value < lag_min) {
      lag_min = value
    }
    if (bridge_count == 1 || value > lag_max) {
      lag_max = value
    }
    lag_values[bridge_count] = value
    if (value >= threshold_ms) {
      threshold_count++
    }
  }
}

/追加介入：busy 入队，等 turn terminal drain/ {
  busy_enqueue_count++
}

END {
  print "== Fuxi Bridge Lag Report =="
  print "log_file: " FILENAME
  print "bridge_forward_events: " bridge_count
  print "busy_enqueue_events: " busy_enqueue_count

  if (bridge_count == 0) {
    print "lag_ms: no_data"
    exit 0
  }

  avg = lag_sum / bridge_count
  n = asort(lag_values)
  if (n % 2 == 1) {
    median = lag_values[(n + 1) / 2]
  } else {
    median = (lag_values[n / 2] + lag_values[n / 2 + 1]) / 2
  }

  print "lag_ms_min: " lag_min
  print "lag_ms_median: " int(median)
  print "lag_ms_avg: " int(avg)
  print "lag_ms_max: " lag_max
  print "lag_ms_ge_3000: " threshold_count
  print "interrupt_first_true: " interrupt_true_count
  print "interrupt_first_false: " interrupt_false_count
}
' "$LOG_FILE"
