#!/usr/bin/env bash
set -euo pipefail

# 解析 bridge 回报链路日志。
# 用法：
#   scripts/bridge-lag-report.sh
#   scripts/bridge-lag-report.sh /tmp/fuxi.log
#   scripts/bridge-lag-report.sh --compare /tmp/fuxi-baseline.log /tmp/fuxi-tuned.log

print_report() {
  local log_file="$1"
  awk '
BEGIN {
  bridge_count = 0
  busy_enqueue_count = 0
  threshold_count = 0
  threshold_ms = 3000
}

/bridge: 转发门客(任务终态回报到玄女|下线回报到玄女)/ {
  bridge_count++
  if (index($0, "interrupt_first=true") > 0) interrupt_true_count++
  if (index($0, "interrupt_first=false") > 0) interrupt_false_count++
  if (match($0, /lag_ms=[0-9]+/)) {
    value = substr($0, RSTART + 7, RLENGTH - 7) + 0
    lag_sum += value
    if (bridge_count == 1 || value < lag_min) lag_min = value
    if (bridge_count == 1 || value > lag_max) lag_max = value
    lag_values[bridge_count] = value
    if (value >= threshold_ms) threshold_count++
  }
}

/追加介入：busy 入队，等 turn terminal drain/ { busy_enqueue_count++ }

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
' "$log_file"
}

extract_metrics() {
  local log_file="$1"
  local prefix="$2"
  awk -v p="$prefix" '
BEGIN {
  bridge_count = 0
  busy_enqueue_count = 0
  threshold_count = 0
  threshold_ms = 3000
}

/bridge: 转发门客(任务终态回报到玄女|下线回报到玄女)/ {
  bridge_count++
  if (index($0, "interrupt_first=true") > 0) interrupt_true_count++
  if (index($0, "interrupt_first=false") > 0) interrupt_false_count++
  if (match($0, /lag_ms=[0-9]+/)) {
    value = substr($0, RSTART + 7, RLENGTH - 7) + 0
    lag_sum += value
    if (bridge_count == 1 || value < lag_min) lag_min = value
    if (bridge_count == 1 || value > lag_max) lag_max = value
    lag_values[bridge_count] = value
    if (value >= threshold_ms) threshold_count++
  }
}

/追加介入：busy 入队，等 turn terminal drain/ { busy_enqueue_count++ }

END {
  print p "_bridge_count=" bridge_count
  print p "_busy_enqueue_count=" busy_enqueue_count
  if (bridge_count == 0) {
    print p "_has_data=0"
    exit 0
  }

  print p "_has_data=1"
  avg = lag_sum / bridge_count
  n = asort(lag_values)
  if (n % 2 == 1) {
    median = lag_values[(n + 1) / 2]
  } else {
    median = (lag_values[n / 2] + lag_values[n / 2 + 1]) / 2
  }

  print p "_lag_min=" lag_min
  print p "_lag_median=" int(median)
  print p "_lag_avg=" int(avg)
  print p "_lag_max=" lag_max
  print p "_lag_ge_3000=" threshold_count
  print p "_interrupt_true=" interrupt_true_count
  print p "_interrupt_false=" interrupt_false_count
}
' "$log_file"
}

if [[ "${1:-}" == "--compare" ]]; then
  base_log="${2:-}"
  tuned_log="${3:-}"
  if [[ -z "$base_log" || -z "$tuned_log" ]]; then
    echo "用法: scripts/bridge-lag-report.sh --compare <baseline.log> <tuned.log>" >&2
    exit 2
  fi
  if [[ ! -f "$base_log" || ! -f "$tuned_log" ]]; then
    echo "日志文件不存在: $base_log 或 $tuned_log" >&2
    exit 2
  fi

  eval "$(extract_metrics "$base_log" base)"
  eval "$(extract_metrics "$tuned_log" tuned)"

  print_report "$base_log"
  echo
  print_report "$tuned_log"
  echo
  echo "== Delta (tuned - baseline) =="
  echo "bridge_forward_events_delta: $((tuned_bridge_count - base_bridge_count))"
  echo "busy_enqueue_events_delta: $((tuned_busy_enqueue_count - base_busy_enqueue_count))"
  if [[ "$base_has_data" == "1" && "$tuned_has_data" == "1" ]]; then
    echo "lag_ms_avg_delta: $((tuned_lag_avg - base_lag_avg))"
    echo "lag_ms_median_delta: $((tuned_lag_median - base_lag_median))"
    echo "lag_ms_max_delta: $((tuned_lag_max - base_lag_max))"
    echo "lag_ms_ge_3000_delta: $((tuned_lag_ge_3000 - base_lag_ge_3000))"
    echo "interrupt_first_true_delta: $((tuned_interrupt_true - base_interrupt_true))"
  else
    echo "lag_delta: no_data"
  fi
  exit 0
fi

log_file="${1:-/tmp/fuxi.log}"
if [[ ! -f "$log_file" ]]; then
  echo "日志文件不存在: $log_file" >&2
  exit 2
fi
print_report "$log_file"
