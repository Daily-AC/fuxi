#!/usr/bin/env bash
set -euo pipefail

# 解析 bridge 回报链路日志。
# 用法：
#   scripts/bridge-lag-report.sh
#   scripts/bridge-lag-report.sh /tmp/fuxi.log
#   scripts/bridge-lag-report.sh --compare /tmp/fuxi-baseline.log /tmp/fuxi-tuned.log

calc_median() {
  local values_file="$1"
  awk '
  { a[NR] = $1 }
  END {
    if (NR == 0) {
      print "0"
      exit 0
    }
    if (NR % 2 == 1) {
      print int(a[(NR + 1) / 2])
    } else {
      print int((a[NR / 2] + a[NR / 2 + 1]) / 2)
    }
  }
  ' "$values_file"
}

emit_metrics() {
  local log_file="$1"
  local prefix="$2"

  local bridge_lines_raw bridge_lines
  bridge_lines_raw="$(grep -E 'bridge: 转发门客(任务终态回报到玄女|下线回报到玄女)' "$log_file" || true)"
  bridge_lines="$(printf '%s\n' "$bridge_lines_raw" | sed -E $'s/\x1B\\[[0-9;]*[[:alpha:]]//g')"

  local bridge_count busy_enqueue_count interrupt_true_count interrupt_false_count
  bridge_count="$(printf '%s\n' "$bridge_lines" | sed '/^$/d' | wc -l | tr -d ' ')"
  busy_enqueue_count="$(grep -c '追加介入：busy 入队，等 turn terminal drain' "$log_file" || true)"
  interrupt_true_count="$(printf '%s\n' "$bridge_lines" | grep -c 'interrupt_first=true' || true)"
  interrupt_false_count="$(printf '%s\n' "$bridge_lines" | grep -c 'interrupt_first=false' || true)"

  echo "${prefix}_bridge_count=${bridge_count}"
  echo "${prefix}_busy_enqueue_count=${busy_enqueue_count}"
  echo "${prefix}_interrupt_true=${interrupt_true_count}"
  echo "${prefix}_interrupt_false=${interrupt_false_count}"

  if [[ "$bridge_count" -eq 0 ]]; then
    echo "${prefix}_has_data=0"
    echo "${prefix}_lag_min=0"
    echo "${prefix}_lag_median=0"
    echo "${prefix}_lag_avg=0"
    echo "${prefix}_lag_max=0"
    echo "${prefix}_lag_ge_3000=0"
    return
  fi

  local tmp_sorted
  tmp_sorted="$(mktemp)"
  printf '%s\n' "$bridge_lines" \
    | grep -oE 'lag_ms=[0-9]+' \
    | sed 's/lag_ms=//' \
    | sort -n > "$tmp_sorted"

  local lag_min lag_max lag_avg lag_median lag_ge_3000
  lag_min="$(head -n 1 "$tmp_sorted")"
  lag_max="$(tail -n 1 "$tmp_sorted")"
  lag_avg="$(awk '{ s += $1 } END { if (NR==0) print 0; else print int(s / NR) }' "$tmp_sorted")"
  lag_median="$(calc_median "$tmp_sorted")"
  lag_ge_3000="$(awk '$1 >= 3000 { c++ } END { print c+0 }' "$tmp_sorted")"

  rm -f "$tmp_sorted"

  echo "${prefix}_has_data=1"
  echo "${prefix}_lag_min=${lag_min}"
  echo "${prefix}_lag_median=${lag_median}"
  echo "${prefix}_lag_avg=${lag_avg}"
  echo "${prefix}_lag_max=${lag_max}"
  echo "${prefix}_lag_ge_3000=${lag_ge_3000}"
}

print_report() {
  local log_file="$1"
  local prefix="$2"

  eval "$(emit_metrics "$log_file" "$prefix")"

  local bridge_count_var="${prefix}_bridge_count"
  local busy_enqueue_count_var="${prefix}_busy_enqueue_count"
  local has_data_var="${prefix}_has_data"
  local lag_min_var="${prefix}_lag_min"
  local lag_median_var="${prefix}_lag_median"
  local lag_avg_var="${prefix}_lag_avg"
  local lag_max_var="${prefix}_lag_max"
  local lag_ge_3000_var="${prefix}_lag_ge_3000"
  local interrupt_true_var="${prefix}_interrupt_true"
  local interrupt_false_var="${prefix}_interrupt_false"

  echo "== Fuxi Bridge Lag Report =="
  echo "log_file: $log_file"
  echo "bridge_forward_events: ${!bridge_count_var}"
  echo "busy_enqueue_events: ${!busy_enqueue_count_var}"

  if [[ "${!has_data_var}" -eq 0 ]]; then
    echo "lag_ms: no_data"
    return
  fi

  echo "lag_ms_min: ${!lag_min_var}"
  echo "lag_ms_median: ${!lag_median_var}"
  echo "lag_ms_avg: ${!lag_avg_var}"
  echo "lag_ms_max: ${!lag_max_var}"
  echo "lag_ms_ge_3000: ${!lag_ge_3000_var}"
  echo "interrupt_first_true: ${!interrupt_true_var}"
  echo "interrupt_first_false: ${!interrupt_false_var}"
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

  eval "$(emit_metrics "$base_log" base)"
  eval "$(emit_metrics "$tuned_log" tuned)"

  print_report "$base_log" base
  echo
  print_report "$tuned_log" tuned
  echo
  echo "== Delta (tuned - baseline) =="
  echo "bridge_forward_events_delta: $((tuned_bridge_count - base_bridge_count))"
  echo "busy_enqueue_events_delta: $((tuned_busy_enqueue_count - base_busy_enqueue_count))"
  if [[ "$base_has_data" -eq 1 && "$tuned_has_data" -eq 1 ]]; then
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
print_report "$log_file" single
